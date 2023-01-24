#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ListFilesResponse {
    #[prost(string, repeated, tag = "1")]
    pub file_names: ::prost::alloc::vec::Vec<::prost::alloc::string::String>,
}
