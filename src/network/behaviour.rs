use libp2p::{identify, ping, rendezvous};
use libp2p::rendezvous::client::Event;
use libp2p::request_response::{RequestResponse, RequestResponseEvent};
use libp2p::swarm::{keep_alive, NetworkBehaviour};
use libp2p_file_get::{FileGet, FileGetEvent};
use void::Void;
use crate::{ListFilesCodec, ListFilesRequest, ListFilesResponse};

#[derive(NetworkBehaviour)]
#[behaviour(out_event = "ComposedEvent")]
pub struct ComposedBehaviour {
    pub identify: identify::Behaviour,
    pub rendezvous: rendezvous::client::Behaviour,
    pub ping: ping::Behaviour,
    pub keep_alive: keep_alive::Behaviour,
    pub list_files: RequestResponse<ListFilesCodec>,
    pub file_get: FileGet,
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum ComposedEvent {
    Rendezvous(Event),
    Identify(identify::Event),
    Ping(ping::Event),
    ListFiles(RequestResponseEvent<ListFilesRequest, ListFilesResponse>),
    FileGet(FileGetEvent),
}

impl From<Event> for ComposedEvent {
    fn from(event: Event) -> Self {
        ComposedEvent::Rendezvous(event)
    }
}

impl From<identify::Event> for ComposedEvent {
    fn from(event: identify::Event) -> Self {
        ComposedEvent::Identify(event)
    }
}

impl From<ping::Event> for ComposedEvent {
    fn from(event: ping::Event) -> Self {
        ComposedEvent::Ping(event)
    }
}

impl From<Void> for ComposedEvent {
    fn from(event: Void) -> Self {
        void::unreachable(event)
    }
}

impl From<RequestResponseEvent<ListFilesRequest, ListFilesResponse>> for ComposedEvent {
    fn from(event: RequestResponseEvent<ListFilesRequest, ListFilesResponse>) -> Self {
        ComposedEvent::ListFiles(event)
    }
}

impl From<FileGetEvent> for ComposedEvent {
    fn from(event: FileGetEvent) -> Self {
        ComposedEvent::FileGet(event)
    }
}

