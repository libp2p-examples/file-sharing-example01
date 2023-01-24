mod handler;
mod protocol;

use core::fmt;
use futures::channel::oneshot;
use futures::{AsyncRead, AsyncWrite};
use handler::{FileGetHandlerEvent, Handler};
use libp2p_core::{connection::ConnectionId, PeerId};
use libp2p_core::{ConnectedPoint, Multiaddr};
use libp2p_swarm::derive_prelude::{
    AddressChange, ConnectionClosed, ConnectionEstablished, DialFailure, FromSwarm,
};
use libp2p_swarm::dial_opts::DialOpts;
use libp2p_swarm::{NetworkBehaviour, NetworkBehaviourAction, NotifyHandler, PollParameters};
use protocol::FileGetOutboundProtocol;
use smallvec::SmallVec;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use thiserror::Error;

mod file_get;
pub use file_get::*;

impl FileGetMessage {
    pub fn new(file_name: String) -> Self {
        FileGetMessage { file_name }
    }
}

/// 发送给应用层的事件
pub enum FileGetEvent {
    /// 收到了入站请求
    RequestReceived {
        /// 对端节点的PeerId
        peer_id: PeerId,
        /// 请求的id
        request_id: RequestId,
        /// 请求消息
        message: FileGetMessage,
        /// 用于发送响应的
        res_sender: oneshot::Sender<Box<dyn AsyncRead + Send + Unpin + 'static>>,
    },
    /// 收到了响应
    ResponseReceived {
        /// 对端节点的PeerId
        peer_id: PeerId,
        /// 请求的id
        request_id: RequestId,
    },
    /// 已经发送了响应
    ResponseSent {
        /// 对端节点的PeerId
        peer_id: PeerId,
        /// 请求的id
        request_id: RequestId,
    },
    /// 出站请求失败
    OutboundFailure {
        /// 对端节点的PeerId
        peer_id: PeerId,
        /// 请求的id
        request_id: RequestId,
        /// 出站错误
        error: OutboundFailure,
    },
    /// 入站请求失败
    InboundFailure {
        /// 对端节点的PeerId
        peer_id: PeerId,
        /// 请求的id
        request_id: RequestId,
        /// 入站错误
        error: InboundFailure,
    },
}

impl fmt::Debug for FileGetEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RequestReceived {
                peer_id,
                request_id,
                message,
                ..
            } => f
                .debug_struct("RequestReceived")
                .field("peer_id", peer_id)
                .field("request_id", request_id)
                .field("message", message)
                .finish(),
            Self::ResponseReceived {
                peer_id,
                request_id,
            } => f
                .debug_struct("ResponseReceived")
                .field("peer_id", peer_id)
                .field("request_id", request_id)
                .finish(),
            Self::ResponseSent {
                peer_id,
                request_id,
            } => f
                .debug_struct("ResponseSent")
                .field("peer_id", peer_id)
                .field("request_id", request_id)
                .finish(),
            Self::OutboundFailure {
                peer_id,
                request_id,
                error,
            } => f
                .debug_struct("OutboundFailure")
                .field("peer_id", peer_id)
                .field("request_id", request_id)
                .field("error", error)
                .finish(),
            Self::InboundFailure {
                peer_id,
                request_id,
                error,
            } => f
                .debug_struct("InboundFailure")
                .field("peer_id", peer_id)
                .field("request_id", request_id)
                .field("error", error)
                .finish(),
        }
    }
}

/// 出站失败
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum OutboundFailure {
    /// 拨号失败
    #[error("Failed to dial the requested peer")]
    DialFailure,
    /// 超时
    #[error("Timeout while waiting for a response")]
    Timeout,
    /// 连接关闭
    #[error("Connection was closed before a response was received")]
    ConnectionClosed,
    /// 不支持的协议
    #[error("The remote supports none of the requested protocols")]
    UnsupportedProtocols,
}

/// 入站失败
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum InboundFailure {
    /// 超时
    #[error("Timeout while receiving request or sending response")]
    Timeout,
    /// 连接关闭
    #[error("Connection was closed before a response could be sent")]
    ConnectionClosed,
    /// 不支持的协议
    #[error("The local peer supports none of the protocols requested by the remote")]
    UnsupportedProtocols,
    /// 省略的响应
    #[error("The response channel was dropped without sending a response to the remote")]
    ResponseOmission,
}

#[derive(Debug, Clone)]
pub struct FileGetConfig {
    pub timeout: Duration,
}

impl FileGetConfig {
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId(u64);

impl From<u64> for RequestId {
    fn from(value: u64) -> Self {
        RequestId(value)
    }
}

#[derive(Debug)]
pub struct RequestIdGenerator(AtomicU64);

impl RequestIdGenerator {
    pub fn new() -> Self {
        RequestIdGenerator(AtomicU64::new(1))
    }

    pub fn next_id(&self) -> RequestId {
        RequestId(self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

pub struct FileGet {
    requset_id_generator: RequestIdGenerator,
    inbound_id_generator: Arc<RequestIdGenerator>,
    config: FileGetConfig,
    connected: HashMap<PeerId, SmallVec<[Connection; 2]>>,
    pending_events: VecDeque<NetworkBehaviourAction<FileGetEvent, Handler>>,
    pending_outbound_requests: HashMap<PeerId, SmallVec<[FileGetOutboundProtocol; 10]>>,
    addresses: HashMap<PeerId, SmallVec<[Multiaddr; 6]>>,
}

impl FileGet {
    pub fn from_config(config: FileGetConfig) -> Self {
        Self {
            requset_id_generator: RequestIdGenerator::new(),
            inbound_id_generator: Arc::new(RequestIdGenerator::new()),
            config,
            connected: Default::default(),
            pending_events: Default::default(),
            pending_outbound_requests: Default::default(),
            addresses: Default::default(),
        }
    }

    /// 发送请求
    pub fn send_request(
        &mut self,
        peer_id: PeerId,
        request: FileGetMessage,
        file_writer: Box<dyn AsyncWrite + Send + Unpin + 'static>,
    ) -> RequestId
    {
        // 生成 request_id
        let request_id = self.requset_id_generator.next_id();

        let request = FileGetOutboundProtocol {
            request_id,
            file_name: request.file_name,
            file_writer,
        };

        // 如果尝试发送失败，说明没有创建好的连接
        if let Some(request) = self.try_send_request(peer_id, request) {
            // 拨号
            let handler = self.new_handler();
            self.pending_events.push_back(NetworkBehaviourAction::Dial {
                opts: DialOpts::from(peer_id),
                handler,
            });
            // 缓存请求，将在连接创建好的时候发送
            self.pending_outbound_requests
                .entry(peer_id)
                .or_default()
                .push(request);
        }

        request_id
    }

    /// 为入站请求发送响应
    pub fn send_response<R>(
        &mut self,
        res_sender: oneshot::Sender<Box<R>>,
        reader: R,
    ) -> Result<(), Box<R>>
    where
        R: AsyncRead + Send + Unpin + 'static,
    {
        res_sender.send(Box::new(reader))
    }

    /// 手动添加地址
    pub fn add_address(&mut self, peer_id: &PeerId, address: Multiaddr) {
        self.addresses.entry(*peer_id).or_default().push(address);
    }

    pub fn remove_address(&mut self, peer_id: &PeerId, address: &Multiaddr) {
        if let Some(addresses) = self.addresses.get_mut(peer_id) {
            addresses.retain(|addr| addr != address);
            if addresses.is_empty() {
                self.addresses.remove(peer_id);
            }
        }
    }

    fn try_send_request(
        &mut self,
        peer_id: PeerId,
        request: FileGetOutboundProtocol,
    ) -> Option<FileGetOutboundProtocol> {
        if let Some(connections) = self.connected.get_mut(&peer_id) {
            if connections.is_empty() {
                return Some(request);
            }
            let idx = (request.request_id.0 as usize) % connections.len();
            let conn = &mut connections[idx];
            conn.pending_inbound_responses.insert(request.request_id);
            self.pending_events
                .push_back(NetworkBehaviourAction::NotifyHandler {
                    peer_id,
                    handler: NotifyHandler::One(conn.id),
                    event: request,
                });
            None
        } else {
            Some(request)
        }
    }

    fn on_connection_established(
        &mut self,
        ConnectionEstablished {
            peer_id,
            connection_id,
            endpoint,
            other_established,
            ..
        }: ConnectionEstablished,
    ) {
        let address = match endpoint {
            ConnectedPoint::Dialer { address, .. } => Some(address.clone()),
            ConnectedPoint::Listener { .. } => None,
        };

        self.connected
            .entry(peer_id)
            .or_default()
            .push(Connection::new(connection_id, address));

        // 如果是第一次创建连接, 把缓存的出站请求发出
        if other_established == 0 {
            if let Some(pending) = self.pending_outbound_requests.remove(&peer_id) {
                for request in pending {
                    self.try_send_request(peer_id, request);
                }
            }
        }
    }

    fn on_connection_closed(
        &mut self,
        ConnectionClosed {
            peer_id,
            connection_id,
            ..
        }: ConnectionClosed<Handler>,
    ) {
        let connections = self
            .connected
            .get_mut(&peer_id)
            .expect("Expected some connections established to peer before closing.");

        let conn = connections
            .iter()
            .position(|c| c.id == connection_id)
            .map(|p| connections.remove(p))
            .expect("Expected connection to be establised before close.");

        if connections.is_empty() {
            self.connected.remove(&peer_id);
        }

        for request_id in conn.pending_outbound_responses {
            self.pending_events
                .push_back(NetworkBehaviourAction::GenerateEvent(
                    FileGetEvent::OutboundFailure {
                        peer_id,
                        request_id,
                        error: OutboundFailure::ConnectionClosed,
                    },
                ))
        }

        for request_id in conn.pending_inbound_responses {
            self.pending_events
                .push_back(NetworkBehaviourAction::GenerateEvent(
                    FileGetEvent::InboundFailure {
                        peer_id,
                        request_id,
                        error: InboundFailure::ConnectionClosed,
                    },
                ))
        }
    }

    fn on_address_change(
        &mut self,
        AddressChange {
            peer_id,
            connection_id,
            new,
            ..
        }: AddressChange,
    ) {
        let new_address = match new {
            ConnectedPoint::Dialer { address, .. } => Some(address.clone()),
            ConnectedPoint::Listener { .. } => None,
        };

        let connections = self
            .connected
            .get_mut(&peer_id)
            .expect("Address change can only happen for established connections.");

        let connection = connections
            .iter_mut()
            .find(|c| c.id == connection_id)
            .expect("Address change can only happen for established connections.");

        connection.address = new_address;
    }

    fn one_dial_failure(&mut self, DialFailure { peer_id, .. }: DialFailure<Handler>) {
        if let Some(peer_id) = peer_id {
            if let Some(pending) = self.pending_outbound_requests.remove(&peer_id) {
                for request in pending {
                    self.pending_events
                        .push_back(NetworkBehaviourAction::GenerateEvent(
                            FileGetEvent::OutboundFailure {
                                peer_id,
                                request_id: request.request_id,
                                error: OutboundFailure::DialFailure,
                            },
                        ));
                }
            }
        }
    }

    fn remove_pending_inbound_response(
        &mut self,
        peer_id: &PeerId,
        connection_id: &ConnectionId,
        request_id: &RequestId,
    ) -> bool {
        self.connected
            .get_mut(peer_id)
            .and_then(|connections| connections.iter_mut().find(|c| c.id == *connection_id))
            .map(|conn| conn.pending_inbound_responses.remove(request_id))
            .unwrap_or(false)
    }

    fn remove_pending_outbound_response(
        &mut self,
        peer_id: &PeerId,
        connection_id: &ConnectionId,
        request_id: &RequestId,
    ) -> bool {
        self.connected
            .get_mut(peer_id)
            .and_then(|connections| connections.iter_mut().find(|c| c.id == *connection_id))
            .map(|conn| conn.pending_outbound_responses.remove(request_id))
            .unwrap_or(false)
    }
}

impl NetworkBehaviour for FileGet {
    type ConnectionHandler = Handler;
    type OutEvent = FileGetEvent;

    fn new_handler(&mut self) -> Self::ConnectionHandler {
        Handler::new(self.config.timeout, self.inbound_id_generator.clone())
    }

    fn on_swarm_event(&mut self, event: FromSwarm<Self::ConnectionHandler>) {
        match event {
            FromSwarm::ConnectionEstablished(connection_established) => {
                self.on_connection_established(connection_established)
            }
            FromSwarm::ConnectionClosed(connection_closed) => {
                self.on_connection_closed(connection_closed)
            }
            FromSwarm::AddressChange(address_change) => self.on_address_change(address_change),
            FromSwarm::DialFailure(dial_failure) => self.one_dial_failure(dial_failure),
            FromSwarm::ListenFailure(_) => {}
            FromSwarm::NewListener(_) => {}
            FromSwarm::NewListenAddr(_) => {}
            FromSwarm::ExpiredListenAddr(_) => {}
            FromSwarm::ListenerError(_) => {}
            FromSwarm::ListenerClosed(_) => {}
            FromSwarm::NewExternalAddr(_) => {}
            FromSwarm::ExpiredExternalAddr(_) => {}
        }
    }

    fn on_connection_handler_event(
        &mut self,
        peer_id: PeerId,
        connection_id: ConnectionId,
        event: <<Self::ConnectionHandler as libp2p_swarm::IntoConnectionHandler>::Handler as
            libp2p_swarm::ConnectionHandler>::OutEvent,
    ) {
        match event {
            FileGetHandlerEvent::Request {
                request_id,
                message,
                res_sender,
            } => {
                self.pending_events
                    .push_back(NetworkBehaviourAction::GenerateEvent(
                        FileGetEvent::RequestReceived {
                            peer_id,
                            request_id,
                            message,
                            res_sender,
                        },
                    ));

                match self
                    .connected
                    .get_mut(&peer_id)
                    .and_then(|connections| connections.iter_mut().find(|c| c.id == connection_id))
                {
                    Some(conn) => {
                        conn.pending_outbound_responses.insert(request_id);
                    }
                    None => {
                        self.pending_events
                            .push_back(NetworkBehaviourAction::GenerateEvent(
                                FileGetEvent::InboundFailure {
                                    peer_id,
                                    request_id,
                                    error: InboundFailure::ConnectionClosed,
                                },
                            ));
                    }
                }
            }
            FileGetHandlerEvent::Response(request_id) => {
                let removed =
                    self.remove_pending_inbound_response(&peer_id, &connection_id, &request_id);
                debug_assert!(
                    removed,
                    "Expect request_id to be pending before receiving response."
                );

                self.pending_events
                    .push_back(NetworkBehaviourAction::GenerateEvent(
                        FileGetEvent::ResponseReceived {
                            peer_id,
                            request_id,
                        },
                    ));
            }
            FileGetHandlerEvent::ResponseSent(request_id) => {
                let removed =
                    self.remove_pending_outbound_response(&peer_id, &connection_id, &request_id);
                debug_assert!(
                    removed,
                    "Expect request_id to be pending before sending response."
                );

                self.pending_events
                    .push_back(NetworkBehaviourAction::GenerateEvent(
                        FileGetEvent::ResponseSent {
                            peer_id,
                            request_id,
                        },
                    ));
            }
            FileGetHandlerEvent::ResponseOmission(request_id) => {
                let removed =
                    self.remove_pending_outbound_response(&peer_id, &connection_id, &request_id);
                debug_assert!(
                    removed,
                    "Expect request_id to be pending before sending response."
                );

                self.pending_events
                    .push_back(NetworkBehaviourAction::GenerateEvent(
                        FileGetEvent::InboundFailure {
                            peer_id,
                            request_id,
                            error: InboundFailure::ResponseOmission,
                        },
                    ));
            }
            FileGetHandlerEvent::OutboundTimeout(request_id) => {
                self.remove_pending_inbound_response(&peer_id, &connection_id, &request_id);

                self.pending_events
                    .push_back(NetworkBehaviourAction::GenerateEvent(
                        FileGetEvent::OutboundFailure {
                            peer_id,
                            request_id,
                            error: OutboundFailure::Timeout,
                        },
                    ));
            }
            FileGetHandlerEvent::OutboundUnsupportedProtocols(request_id) => {
                self.remove_pending_inbound_response(&peer_id, &connection_id, &request_id);

                self.pending_events
                    .push_back(NetworkBehaviourAction::GenerateEvent(
                        FileGetEvent::OutboundFailure {
                            peer_id,
                            request_id,
                            error: OutboundFailure::UnsupportedProtocols,
                        },
                    ));
            }
            FileGetHandlerEvent::InboundTimeout(request_id) => {
                self.remove_pending_outbound_response(&peer_id, &connection_id, &request_id);

                self.pending_events
                    .push_back(NetworkBehaviourAction::GenerateEvent(
                        FileGetEvent::InboundFailure {
                            peer_id,
                            request_id,
                            error: InboundFailure::Timeout,
                        },
                    ));
            }
            FileGetHandlerEvent::InboundUnsupportedProtocols(request_id) => {
                self.remove_pending_outbound_response(&peer_id, &connection_id, &request_id);

                self.pending_events
                    .push_back(NetworkBehaviourAction::GenerateEvent(
                        FileGetEvent::InboundFailure {
                            peer_id,
                            request_id,
                            error: InboundFailure::UnsupportedProtocols,
                        },
                    ));
            }
        }
    }

    fn poll(
        &mut self,
        _cx: &mut Context<'_>,
        _params: &mut impl PollParameters,
    ) -> Poll<NetworkBehaviourAction<Self::OutEvent, Self::ConnectionHandler>> {
        if let Some(event) = self.pending_events.pop_front() {
            return Poll::Ready(event);
        } else if self.pending_events.capacity() > EMPTY_QUEUE_SHRINK_THRESHOLD {
            self.pending_events.shrink_to_fit();
        }

        Poll::Pending
    }
}

const EMPTY_QUEUE_SHRINK_THRESHOLD: usize = 100;

struct Connection {
    id: ConnectionId,
    address: Option<Multiaddr>,
    pending_outbound_responses: HashSet<RequestId>,
    pending_inbound_responses: HashSet<RequestId>,
}

impl Connection {
    fn new(id: ConnectionId, address: Option<Multiaddr>) -> Self {
        Self {
            id,
            address,
            pending_inbound_responses: Default::default(),
            pending_outbound_responses: Default::default(),
        }
    }
}
