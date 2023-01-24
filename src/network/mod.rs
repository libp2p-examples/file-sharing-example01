use futures::prelude::*;
use futures_timer::Delay;
use libp2p_file_get::{FileGet, FileGetConfig, FileGetEvent, FileGetMessage};
use std::collections::HashMap;
use std::io::Error;
use std::iter;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::fs::File;
use tokio_stream::wrappers::ReadDirStream;
use tokio_util::compat::TokioAsyncReadCompatExt;

use crate::{
    Command, CommandRequest, CommandResponse, CommandResult, FileSharingError, GetFile, ListFiles,
    Register,
};
use libp2p::core::either::EitherError;
use libp2p::futures::channel::{mpsc, oneshot};
use libp2p::identity::Keypair;
use libp2p::ping::Failure;
use libp2p::rendezvous::Namespace;
use libp2p::request_response::{
    ProtocolSupport, RequestId as ReqResRequestId, RequestResponse, RequestResponseEvent,
    RequestResponseMessage,
};
use libp2p::swarm::{keep_alive, ConnectionHandlerUpgrErr, SwarmEvent};
use libp2p::{identify, ping, rendezvous, tokio_development_transport, Multiaddr, Swarm};
use libp2p::{identity::ed25519::SecretKey, PeerId};
use libp2p_file_get::RequestId as FileGetRequestId;
use rendezvous::client::Event;
use void::Void;

mod abi;
mod behaviour;
mod protocol;

pub use abi::*;
pub use behaviour::*;
pub use protocol::*;

#[derive(Debug, Default)]
struct CommandRequestIdGenerator(AtomicU64);

impl CommandRequestIdGenerator {
    fn next_id(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}

pub struct Client {
    command_request_sender: mpsc::Sender<CommandRequest>,
    command_request_id_generator: CommandRequestIdGenerator,
}

impl Client {
    pub fn new(command_request_sender: mpsc::Sender<CommandRequest>) -> Self {
        Self {
            command_request_sender,
            command_request_id_generator: CommandRequestIdGenerator::default(),
        }
    }

    pub async fn send_command(
        &mut self,
        command: Command,
    ) {
        let (response_sender, response_receiver) =
            oneshot::channel::<Result<CommandResponse, FileSharingError>>();

        let command_request = CommandRequest {
            command_request_id: self.command_request_id_generator.next_id(),
            command,
            response_sender,
        };

        let _ = self.command_request_sender.send(command_request).await;

        match response_receiver.await.unwrap() {
            Ok(response) => println!("{response:?}"),
            Err(e) => eprintln!("error: {e:?}"),
        }
    }
}

/// 事件循环
pub struct EventLoop {
    /// Swarm
    swarm: Swarm<ComposedBehaviour>,
    /// Rendezvous point
    rendezvous_point_address: Multiaddr,
    /// Rendezvous peer_id
    rendezvous_point_peer_id: Option<PeerId>,
    /// 命令接收器
    command_request_receiver: mpsc::Receiver<CommandRequest>,
    /// Namespace 与 peer_id 的映射
    namespace_peers: HashMap<Namespace, PeerId>,
    /// 正在处理的 Command
    pending_command_requests: HashMap<u64, CommandRequest>,
    /// 正在处理的 List Files
    pending_list_files: HashMap<ReqResRequestId, u64>,
    /// 正在处理的 File Get 请求
    pending_file_gets: HashMap<FileGetRequestId, u64>,
    /// 共享的文件路径
    sharing_dir: PathBuf,
}

impl EventLoop {
    pub fn new(
        swarm: Swarm<ComposedBehaviour>,
        rendezvous_point_address: Multiaddr,
        command_request_receiver: mpsc::Receiver<CommandRequest>,
        sharing_dir: PathBuf,
    ) -> Self {
        Self {
            swarm,
            rendezvous_point_address,
            rendezvous_point_peer_id: None,
            command_request_receiver,
            namespace_peers: Default::default(),
            pending_command_requests: Default::default(),
            pending_list_files: Default::default(),
            pending_file_gets: Default::default(),
            sharing_dir,
        }
    }

    /// 初始化
    pub async fn init(mut self) -> Result<Self, FileSharingError> {
        // 开始监听
        self.start_listen().await?;
        // 连接到 rendezvous point
        self.conntect_rendezvous().await?;

        Ok(self)
    }

    /// 连接到 rendezvous point
    async fn conntect_rendezvous(&mut self) -> Result<(), FileSharingError> {
        self.swarm.dial(self.rendezvous_point_address.clone())?;
        let delay = Delay::new(Duration::from_secs(3)).fuse();
        tokio::pin!(delay);
        loop {
            tokio::select! {
                event = self.swarm.select_next_some() => {
                    if let SwarmEvent::Behaviour(ComposedEvent::Identify(identify::Event::Received { peer_id, .. })) = event {
                        self.rendezvous_point_peer_id = Some(peer_id);
                        break;
                    }
                },
                _ = &mut delay => {
                    break;
                }
            }
        }
        match self.rendezvous_point_peer_id.is_some() {
            true => {
                tracing::info!("Rendezvous point connected");
                Ok(())
            }
            false => Err(FileSharingError::RendezvousPointNotConnected),
        }
    }

    /// 开始监听
    async fn start_listen(&mut self) -> Result<(), FileSharingError> {
        let local_address: Multiaddr = "/ip4/0.0.0.0/tcp/0".parse().unwrap();
        let _ = self.swarm.listen_on(local_address.clone());
        let mut listen_success = false;
        let delay = Delay::new(Duration::from_secs(3)).fuse();
        tokio::pin!(delay);
        loop {
            tokio::select! {
                event = self.swarm.select_next_some() => {
                    match event {
                        SwarmEvent::NewListenAddr { address, .. } => {
                            tracing::info!("Listening on {}", address);
                            listen_success = true;
                            break;
                        }
                        SwarmEvent::ListenerError { error, .. } => {
                            tracing::info!("Listen error {}", error);
                            listen_success = false;
                            break;
                        }
                        _ => {}
                    }
                },
                _ = &mut delay => {
                    break;
                }
            }
        }
        match listen_success {
            true => {
                tracing::info!("Listening on {}", local_address);
                Ok(())
            }
            false => Err(FileSharingError::ListenError(local_address)),
        }
    }

    /// 运行事件循环
    pub async fn run(mut self) -> Result<(), FileSharingError> {
        // let mut discover_tick = tokio::time::interval(Duration::from_secs(30));
        loop {
            tokio::select! {
                command_request = self.command_request_receiver.select_next_some() => self.handle_command(command_request).await?,
                event = self.swarm.select_next_some() => self.handle_event(event).await?,
            }
        }
    }

    /// 处理命令
    pub async fn handle_command(
        &mut self,
        command_request: CommandRequest,
    ) -> Result<(), FileSharingError> {
        let CommandRequest {
            command,
            response_sender,
            command_request_id,
        } = command_request;

        match command {
            Command::Register(Register { user_name }) => {
                self.swarm.behaviour_mut().rendezvous.register(
                    Namespace::new(user_name.clone())?,
                    self.rendezvous_point_peer_id
                        .expect("Rendezvous point peer id has been set before register"),
                    None,
                );
                let response = CommandResponse::new(command_request_id, CommandResult::Register);
                let _ = response_sender.send(Ok(response));
            }
            Command::ListPeers => {
                self.swarm.behaviour_mut().rendezvous.discover(
                    None,
                    None,
                    None,
                    self.rendezvous_point_peer_id
                        .expect("Rendezvous point peer id has been set before register"),
                );
                self.pending_command_requests.insert(
                    command_request_id,
                    CommandRequest {
                        command_request_id,
                        command: Command::ListPeers,
                        response_sender,
                    },
                );
            }
            Command::ListFiles(ListFiles { user_name }) => {
                // 检查是否存在对应 namespace 记录，如果没有，视为用户没有上线
                match self.get_peer_id(&user_name) {
                    Some(peer_id) => {
                        // 发送请求
                        let request = ListFilesRequest {};
                        let request_id = self
                            .swarm
                            .behaviour_mut()
                            .list_files
                            .send_request(&peer_id, request);

                        // 把 CommandRequest 缓存起来，等到响应到达以后，把结果写回到 CommandResponse
                        self.pending_command_requests.insert(
                            command_request_id,
                            CommandRequest {
                                command: Command::ListFiles(ListFiles { user_name }),
                                response_sender,
                                command_request_id,
                            },
                        );
                        self.pending_list_files
                            .insert(request_id, command_request_id);
                    }
                    None => {
                        let _ = response_sender
                            .send(Err(FileSharingError::UserNotRegistered(user_name.clone())));
                    }
                }
            }
            Command::GetFile(GetFile {
                user_name,
                file_name,
            }) => {
                let request = FileGetMessage::new(file_name.clone());
                let mut file_path = self.sharing_dir.clone();
                file_path.push(file_name.clone());
                let writer = tokio::fs::OpenOptions::new().create(true).write(true).open(file_path).await?;
                match self.get_peer_id(&user_name) {
                    Some(peer_id) => {
                        let request_id = self
                            .swarm
                            .behaviour_mut()
                            .file_get
                            .send_request(peer_id, request, Box::new(writer.compat()));
                        // 把 CommandRequest 缓存起来，等到响应到达以后，把结果写回到 CommandResponse
                        self.pending_command_requests.insert(
                            command_request_id,
                            CommandRequest {
                                command_request_id,
                                command: Command::GetFile(GetFile {
                                    user_name,
                                    file_name,
                                }),
                                response_sender,
                            },
                        );
                        self.pending_file_gets
                            .insert(request_id, command_request_id);
                    }
                    None => {
                        let _ = response_sender
                            .send(Err(FileSharingError::UserNotRegistered(user_name.clone())));
                    }
                }
            }
        }

        Ok(())
    }

    /// 处理事件
    #[allow(clippy::type_complexity)]
    pub async fn handle_event(
        &mut self,
        event: SwarmEvent<
            ComposedEvent,
            EitherError<
                EitherError<
                    EitherError<EitherError<EitherError<Error, Void>, Failure>, Void>,
                    ConnectionHandlerUpgrErr<Error>,
                >,
                ConnectionHandlerUpgrErr<Error>,
            >,
        >,
    ) -> Result<(), FileSharingError> {
        match event {
            SwarmEvent::NewListenAddr { address, .. } => {
                tracing::info!("Listening on {}", address);
            }
            SwarmEvent::Behaviour(ComposedEvent::Rendezvous(Event::Registered {
                namespace,
                ttl,
                rendezvous_node,
            })) => {
                tracing::info!(
                    "Registered for namespace '{}' at rendezvous point {} for the next {} seconds",
                    namespace,
                    rendezvous_node,
                    ttl
                );
            }
            SwarmEvent::Behaviour(ComposedEvent::Rendezvous(Event::RegisterFailed(error))) => {
                tracing::error!("Register failed");
                tracing::error!("{}", error);
            }
            SwarmEvent::Behaviour(ComposedEvent::Rendezvous(Event::DiscoverFailed { .. })) => {
                tracing::error!("Discover failed");
            }
            SwarmEvent::Behaviour(ComposedEvent::Rendezvous(Event::Expired { .. })) => {
                tracing::error!("Expired");
            }
            SwarmEvent::Behaviour(ComposedEvent::Rendezvous(Event::Discovered {
                registrations,
                ..
            })) => {
                for registration in registrations {
                    for address in registration.record.addresses() {
                        let peer = registration.record.peer_id();
                        tracing::info!("Discovered peer {} at {}", peer, address);

                        self.namespace_peers
                            .insert(registration.namespace.clone(), peer.clone());
                    }
                }

                let mut to_remove = Vec::new();
                for (key, value) in self.pending_command_requests.iter() {
                    if matches!(
                        value,
                        CommandRequest {
                            command: Command::ListPeers,
                            ..
                        }
                    ) {
                        to_remove.push(*key);
                    }
                }

                for key in to_remove {
                    if let Some(command) = self.pending_command_requests.remove(&key) {
                        let _ = command.response_sender.send(Ok(CommandResponse {
                            command_request_id: command.command_request_id,
                            result: CommandResult::ListPeers(
                                self.namespace_peers
                                    .keys()
                                    .cloned()
                                    .map(|n| n.to_string())
                                    .collect(),
                            ),
                        }));
                    }
                }
            }
            SwarmEvent::Behaviour(ComposedEvent::ListFiles(RequestResponseEvent::Message {
                message,
                ..
            })) => match message {
                RequestResponseMessage::Request { channel, .. } => {
                    let dir = tokio::fs::read_dir(&self.sharing_dir).await?;
                    let file_names = ReadDirStream::new(dir)
                        .try_filter(|entry| future::ready(!entry.path().is_dir()))
                        .map_ok(|entry| entry.file_name().to_string_lossy().to_string())
                        .try_collect::<Vec<String>>()
                        .await?;

                    let response = ListFilesResponse { file_names };
                    let _ = self
                        .swarm
                        .behaviour_mut()
                        .list_files
                        .send_response(channel, response);
                }
                RequestResponseMessage::Response {
                    request_id,
                    response,
                } => {
                    if let Some(command_request_id) = self.pending_list_files.remove(&request_id) {
                        if let Some(command_request) =
                            self.pending_command_requests.remove(&command_request_id)
                        {
                            let _ = command_request.response_sender.send(Ok(CommandResponse {
                                command_request_id,
                                result: CommandResult::ListFiles(response.file_names),
                            }));
                        }
                    }
                }
            }
            SwarmEvent::Behaviour(ComposedEvent::ListFiles(RequestResponseEvent::OutboundFailure { peer, request_id, error })) => {
                tracing::error!("Outbound failure for peer {} and request id {}: {}", peer, request_id, error);
            }
            SwarmEvent::Behaviour(ComposedEvent::ListFiles(RequestResponseEvent::InboundFailure { peer, request_id, error })) => {
                tracing::error!("Inbound failure for peer {} and request id {}: {}", peer, request_id, error);
            }
            SwarmEvent::Behaviour(ComposedEvent::FileGet(FileGetEvent::RequestReceived {
                message,
                res_sender,
                ..
            })) => {
                let file_path = PathBuf::from(&self.sharing_dir).join(message.file_name);
                let file = File::open(file_path).await?;
                let _ = res_sender.send(Box::new(file.compat()));
            }
            SwarmEvent::Behaviour(ComposedEvent::FileGet(FileGetEvent::ResponseReceived {
                request_id,
                ..
            })) => {
                let command_request_id = self
                    .pending_file_gets
                    .remove(&request_id)
                    .expect("Expect command request to be pending before response is received.");

                let command_request = self
                    .pending_command_requests
                    .remove(&command_request_id)
                    .expect("Expect command request to be pending before response is received.");

                if let Command::GetFile(GetFile {
                    user_name,
                    file_name,
                }) = command_request.command
                {
                    let _ = command_request.response_sender.send(Ok(CommandResponse {
                        command_request_id,
                        result: CommandResult::GetFile {
                            user_name,
                            file_name,
                        },
                    }));
                } else if cfg!(debug_assertions) {
                    panic!("Expect command to be GetFile");
                }
            }
            _ => { }
        }
        Ok(())
    }

    fn get_peer_id(&self, user_name: &str) -> Option<PeerId> {
        let namespace = Namespace::new(user_name.to_string()).ok()?;
        self.namespace_peers.get(&namespace).cloned()
    }
}

pub async fn new_network(
    rendezvous_point_address: Multiaddr,
    secret_key_seed: Option<u8>,
    sharing_dir: PathBuf,
) -> Result<(Client, EventLoop), FileSharingError> {
    let keypair = match secret_key_seed {
        Some(seed) => {
            let mut bytes = [0u8; 32];
            bytes[0] = seed;
            let secret_key = SecretKey::from_bytes(&mut bytes)
                .expect("Only occur when the length is incorrect, no problem here.");
            Keypair::Ed25519(secret_key.into())
        }
        None => Keypair::generate_ed25519(),
    };

    let local_peer_id = keypair.public().to_peer_id();

    let swarm = Swarm::with_tokio_executor(
        tokio_development_transport(keypair.clone()).unwrap(),
        ComposedBehaviour {
            identify: identify::Behaviour::new(identify::Config::new(
                "rendezvous-example/1.0.0".to_string(),
                keypair.public(),
            )),
            rendezvous: rendezvous::client::Behaviour::new(keypair.clone()),
            ping: ping::Behaviour::new(ping::Config::new().with_interval(Duration::from_secs(1))),
            keep_alive: keep_alive::Behaviour,
            list_files: RequestResponse::new(
                ListFilesCodec(),
                iter::once((ListFilesProtocol(), ProtocolSupport::Full)),
                Default::default(),
            ),
            file_get: FileGet::from_config(FileGetConfig::new(Duration::from_secs(60 * 30))),
        },
        local_peer_id,
    );

    let (command_sender, command_receiver) = mpsc::channel(0);

    let client = Client::new(command_sender);

    let event_loop = EventLoop::new(
        swarm,
        rendezvous_point_address,
        command_receiver,
        sharing_dir,
    )
    .init()
    .await?;

    Ok((client, event_loop))
}
