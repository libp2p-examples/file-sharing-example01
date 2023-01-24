use futures::channel::oneshot;
use futures::channel::oneshot::Canceled;
use futures::future::BoxFuture;
use futures::prelude::*;
use futures::stream::FuturesUnordered;
use libp2p_swarm::handler::{
    ConnectionEvent, DialUpgradeError, FullyNegotiatedInbound, FullyNegotiatedOutbound,
    ListenUpgradeError,
};
use libp2p_swarm::{
    ConnectionHandler, ConnectionHandlerEvent, ConnectionHandlerUpgrErr, KeepAlive,
    SubstreamProtocol,
};
use std::collections::VecDeque;
use std::fmt::Debug;
use std::io;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use crate::protocol::{FileGetInboundProtocol, FileGetOutboundProtocol};
use crate::{FileGetMessage, RequestId, RequestIdGenerator, EMPTY_QUEUE_SHRINK_THRESHOLD};

pub struct Handler {
    timeout: Duration,
    inbound_id_generator: Arc<RequestIdGenerator>,
    keep_alive: KeepAlive,
    pending_events: VecDeque<FileGetHandlerEvent>,
    inbound: FuturesUnordered<
        BoxFuture<
            'static,
            Result<
                (
                    (RequestId, FileGetMessage),
                    oneshot::Sender<Box<dyn AsyncRead + Send + Unpin + 'static>>,
                ),
                Canceled,
            >,
        >,
    >,
    outbound: VecDeque<FileGetOutboundProtocol>,
    pending_error: Option<ConnectionHandlerUpgrErr<io::Error>>,
}

impl Handler {
    pub fn new(timeout: Duration, inbound_id_generator: Arc<RequestIdGenerator>) -> Self {
        Handler {
            timeout,
            inbound_id_generator,
            keep_alive: KeepAlive::Yes,
            pending_events: Default::default(),
            inbound: Default::default(),
            outbound: Default::default(),
            pending_error: None,
        }
    }
}

/// 发送给Behaviour的事件
pub enum FileGetHandlerEvent {
    /// 收到了一个请求
    Request {
        request_id: RequestId,
        message: FileGetMessage,
        res_sender: oneshot::Sender<Box<dyn AsyncRead + Send + Unpin + 'static>>,
    },
    /// 收到了一个响应
    Response(RequestId),
    /// 响应已经成功发出
    ResponseSent(RequestId),
    /// 响应没有发送完整
    ResponseOmission(RequestId),
    /// 出站超时
    OutboundTimeout(RequestId),
    /// 出站未协商成功
    OutboundUnsupportedProtocols(RequestId),
    /// 入站超时
    InboundTimeout(RequestId),
    /// 入站未协商成功
    InboundUnsupportedProtocols(RequestId),
}

impl Debug for FileGetHandlerEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Request { message, .. } => {
                f.debug_struct("Request").field("message", message).finish()
            }
            Self::Response(arg0) => f.debug_tuple("Response").field(arg0).finish(),
            Self::ResponseSent(arg0) => f.debug_tuple("ResponseSent").field(arg0).finish(),
            Self::ResponseOmission(arg0) => f.debug_tuple("ResponseOmission").field(arg0).finish(),
            Self::OutboundTimeout(arg0) => f.debug_tuple("OutboundTimeout").field(arg0).finish(),
            Self::OutboundUnsupportedProtocols(arg0) => f
                .debug_tuple("OutboundUnsupportedProtocols")
                .field(arg0)
                .finish(),
            Self::InboundTimeout(arg0) => f.debug_tuple("InboundTimeout").field(arg0).finish(),
            Self::InboundUnsupportedProtocols(arg0) => f
                .debug_tuple("InboundUnsupportedProtocols")
                .field(arg0)
                .finish(),
        }
    }
}

impl ConnectionHandler for Handler {
    type InEvent = FileGetOutboundProtocol;
    type OutEvent = FileGetHandlerEvent;
    type Error = ConnectionHandlerUpgrErr<io::Error>;
    type InboundProtocol = FileGetInboundProtocol;
    type OutboundProtocol = FileGetOutboundProtocol;
    type OutboundOpenInfo = RequestId;
    type InboundOpenInfo = RequestId;

    fn listen_protocol(&self) -> SubstreamProtocol<Self::InboundProtocol, Self::InboundOpenInfo> {
        let request_id = self.inbound_id_generator.next_id();
        let (req_sender, req_receiver) = oneshot::channel::<(RequestId, FileGetMessage)>();
        let (res_sender, res_receiver) = oneshot::channel();
        let inbound_protocol = FileGetInboundProtocol::new(req_sender, res_receiver, request_id);

        self.inbound.push(
            req_receiver
                .map_ok(|message| ((message.0, message.1), res_sender))
                .boxed(),
        );
        SubstreamProtocol::new(inbound_protocol, request_id).with_timeout(self.timeout)
    }

    fn on_behaviour_event(&mut self, request: Self::InEvent) {
        self.keep_alive = KeepAlive::Yes;
        self.outbound.push_back(request);
    }

    fn on_connection_event(
        &mut self,
        event: ConnectionEvent<
            Self::InboundProtocol,
            Self::OutboundProtocol,
            Self::InboundOpenInfo,
            Self::OutboundOpenInfo,
        >,
    ) {
        match event {
            ConnectionEvent::FullyNegotiatedInbound(FullyNegotiatedInbound {
                protocol: sent,
                info: request_id,
            }) => match sent {
                true => self
                    .pending_events
                    .push_back(FileGetHandlerEvent::ResponseSent(request_id)),
                false => self
                    .pending_events
                    .push_back(FileGetHandlerEvent::ResponseOmission(request_id)),
            },

            ConnectionEvent::FullyNegotiatedOutbound(FullyNegotiatedOutbound {
                info: request_id,
                ..
            }) => self
                .pending_events
                .push_back(FileGetHandlerEvent::Response(request_id)),

            ConnectionEvent::ListenUpgradeError(ListenUpgradeError {
                error,
                info: request_id,
            }) => match error {
                ConnectionHandlerUpgrErr::Timeout => self
                    .pending_events
                    .push_back(FileGetHandlerEvent::InboundTimeout(request_id)),

                ConnectionHandlerUpgrErr::Upgrade(_) => self
                    .pending_events
                    .push_back(FileGetHandlerEvent::InboundUnsupportedProtocols(request_id)),

                _ => self.pending_error = Some(error),
            },

            ConnectionEvent::DialUpgradeError(DialUpgradeError {
                error,
                info: request_id,
            }) => match error {
                ConnectionHandlerUpgrErr::Timeout => self
                    .pending_events
                    .push_back(FileGetHandlerEvent::OutboundTimeout(request_id)),
                ConnectionHandlerUpgrErr::Upgrade(_) => self.pending_events.push_back(
                    FileGetHandlerEvent::OutboundUnsupportedProtocols(request_id),
                ),
                _ => self.pending_error = Some(error),
            },

            ConnectionEvent::AddressChange(_) => {}
        }
    }

    fn connection_keep_alive(&self) -> KeepAlive {
        self.keep_alive
    }

    fn poll(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<
        ConnectionHandlerEvent<
            Self::OutboundProtocol,
            Self::OutboundOpenInfo,
            Self::OutEvent,
            Self::Error,
        >,
    > {
        if let Some(error) = self.pending_error.take() {
            return Poll::Ready(ConnectionHandlerEvent::Close(error));
        }

        if let Some(event) = self.pending_events.pop_front() {
            return Poll::Ready(ConnectionHandlerEvent::Custom(event));
        } else if self.pending_events.len() > EMPTY_QUEUE_SHRINK_THRESHOLD {
            self.pending_events.shrink_to_fit();
        }

        if let Some(request) = self.outbound.pop_front() {
            let request_id = request.request_id;
            return Poll::Ready(ConnectionHandlerEvent::OutboundSubstreamRequest {
                protocol: SubstreamProtocol::new(request, request_id).with_timeout(self.timeout),
            });
        } else if self.outbound.len() > EMPTY_QUEUE_SHRINK_THRESHOLD {
            self.outbound.shrink_to_fit();
        }

        if let Poll::Ready(Some(Ok(received))) = self.inbound.poll_next_unpin(cx) {
            self.keep_alive = KeepAlive::Yes;
            return Poll::Ready(ConnectionHandlerEvent::Custom(
                FileGetHandlerEvent::Request {
                    request_id: received.0 .0,
                    message: received.0 .1,
                    res_sender: received.1,
                },
            ));
        }

        if self.inbound.is_empty() && self.keep_alive.is_yes() {
            let until = Instant::now() + self.timeout * 2;
            self.keep_alive = KeepAlive::Until(until);
        }

        Poll::Pending
    }
}
