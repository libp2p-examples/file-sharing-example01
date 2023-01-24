use libp2p::Multiaddr;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum FileSharingError {
    #[error("Invalid command")]
    InvalidCommand,
    #[error("Io error: {0:?}")]
    Io(#[from] std::io::Error),
    #[error("Dial Error : {0:?}")]
    DialError(#[from] libp2p::swarm::DialError),
    #[error("Cannot connect rendezvous point")]
    RendezvousPointNotConnected,
    #[error("Cannot parse rendezvous namespace")]
    NamespaceTooLong(#[from] libp2p::rendezvous::NamespaceTooLong),
    #[error("Cannot listen on local address")]
    ListenError(Multiaddr),
    #[error("User `{0}` is not registered")]
    UserNotRegistered(String),
}