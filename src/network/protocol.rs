use std::io::{self, Error, ErrorKind};

use async_trait::async_trait;
use futures::prelude::*;
use libp2p::bytes::BytesMut;
use libp2p::core::ProtocolName;
use libp2p::request_response::RequestResponseCodec;
use prost::Message;
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};
use tokio_util::compat::{FuturesAsyncReadCompatExt, FuturesAsyncWriteCompatExt};

use crate::ListFilesResponse;

#[derive(Debug, Clone)]
pub struct ListFilesProtocol();
#[derive(Clone)]
pub struct ListFilesCodec();
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListFilesRequest;

impl ProtocolName for ListFilesProtocol {
    fn protocol_name(&self) -> &[u8] {
        "/list-files/1".as_bytes()
    }
}

#[async_trait]
impl RequestResponseCodec for ListFilesCodec {
    type Protocol = ListFilesProtocol;
    type Request = ListFilesRequest;
    type Response = ListFilesResponse;

    async fn read_request<T>(
        &mut self,
        _: &ListFilesProtocol,
        _io: &mut T,
    ) -> io::Result<Self::Request>
    where
        T: AsyncRead + Unpin + Send,
    {
        Ok(ListFilesRequest)
    }

    async fn read_response<T>(
        &mut self,
        _: &ListFilesProtocol,
        io: &mut T,
    ) -> io::Result<Self::Response>
    where
        T: AsyncRead + Unpin + Send,
    {
        let mut reader = FramedRead::new(io.compat(), LengthDelimitedCodec::new());
        if let Some(buf) = reader.next().await {
            let buf = buf?;
            ListFilesResponse::decode(buf).map_err(|_| Error::from(ErrorKind::UnexpectedEof))
        } else {
            Err(Error::from(ErrorKind::UnexpectedEof))
        }
    }

    async fn write_request<T>(
        &mut self,
        _: &ListFilesProtocol,
        _io: &mut T,
        _: ListFilesRequest,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        Ok(())
    }

    async fn write_response<T>(
        &mut self,
        _: &ListFilesProtocol,
        io: &mut T,
        response: ListFilesResponse,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let mut writer = FramedWrite::new(io.compat_write(), LengthDelimitedCodec::new());
        let mut buf = BytesMut::with_capacity(response.encoded_len());
        response.encode(&mut buf)?;
        writer.send(buf.freeze()).await?;

        Ok(())
    }
}
