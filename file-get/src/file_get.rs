#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct FileGetMessage {
    #[prost(string, tag = "1")]
    pub file_name: ::prost::alloc::string::String,
}
