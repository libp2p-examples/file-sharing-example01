use std::fmt;
use std::iter;

use futures::channel::oneshot;
use futures::future::BoxFuture;
use futures::io;
use futures::prelude::*;
use libp2p_core::{InboundUpgrade, OutboundUpgrade, UpgradeInfo};
use libp2p_swarm::NegotiatedSubstream;
use prost::Message;

use crate::FileGetMessage;
use crate::RequestId;

const PROTOCOL_NAME: &[u8] = "/file-get/1.0.0".as_bytes();

pub struct FileGetOutboundProtocol {
    pub request_id: RequestId,
    pub file_name: String,
    pub file_writer: Box<dyn AsyncWrite + Send + Unpin + 'static>,
}

impl fmt::Debug for FileGetOutboundProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileGetOutboundProtocol")
            .field("request_id", &self.request_id)
            .field("file_name", &self.file_name)
            .finish()
    }
}

impl UpgradeInfo for FileGetOutboundProtocol {
    type Info = &'static [u8];
    type InfoIter = iter::Once<Self::Info>;

    fn protocol_info(&self) -> Self::InfoIter {
        iter::once(PROTOCOL_NAME)
    }
}

impl OutboundUpgrade<NegotiatedSubstream> for FileGetOutboundProtocol {
    type Output = ();
    type Error = io::Error;
    type Future = BoxFuture<'static, Result<Self::Output, Self::Error>>;

    fn upgrade_outbound(mut self, mut socket: NegotiatedSubstream, _info: Self::Info) -> Self::Future {
        async move {
            let file_get_message = FileGetMessage {
                file_name: self.file_name,
            };

            send_request(file_get_message, &mut socket).await?;
            copy_stream(&mut socket, &mut self.file_writer).await?;
            Ok(())
        }
        .boxed()
    }
}

pub struct FileGetInboundProtocol {
    request_sender: oneshot::Sender<(RequestId, FileGetMessage)>,
    response_receiver: oneshot::Receiver<Box<dyn AsyncRead + Unpin + Send + 'static>>,
    request_id: RequestId,
}

impl FileGetInboundProtocol {
    pub fn new(
        request_sender: oneshot::Sender<(RequestId, FileGetMessage)>,
        response_receiver: oneshot::Receiver<Box<dyn AsyncRead + Unpin + Send + 'static>>,
        request_id: RequestId,
    ) -> Self {
        Self {
            request_sender,
            response_receiver,
            request_id,
        }
    }
}

impl UpgradeInfo for FileGetInboundProtocol {
    type Info = &'static [u8];
    type InfoIter = iter::Once<Self::Info>;

    fn protocol_info(&self) -> Self::InfoIter {
        iter::once(PROTOCOL_NAME)
    }
}

impl InboundUpgrade<NegotiatedSubstream> for FileGetInboundProtocol {
    type Output = bool;
    type Error = io::Error;
    type Future = BoxFuture<'static, Result<Self::Output, Self::Error>>;
    fn upgrade_inbound(self, mut socket: NegotiatedSubstream, _info: Self::Info) -> Self::Future {
        Box::pin(async move {
            let file_get_message = receive_message(&mut socket).await?;
            let _ = self
                .request_sender
                .send((self.request_id, file_get_message));

            let mut reader = self.response_receiver.await.expect("Sender exists");
            copy_stream(&mut reader, &mut socket).await?;
            Ok(true)
        })
    }
}

async fn send_request<W>(file_get_message: FileGetMessage, socket: &mut W) -> Result<(), io::Error>
where
    W: AsyncWrite + Unpin,
{
    #[cfg(feature = "tokio")]
    tokio_send_request(file_get_message, socket).await
}

#[cfg(feature = "tokio")]
async fn tokio_send_request<W>(
    file_get_message: FileGetMessage,
    writer: &mut W,
) -> Result<(), io::Error>
where
    W: AsyncWrite + Unpin,
{
    use bytes::BytesMut;
    use tokio_util::{
        codec::{FramedWrite, LengthDelimitedCodec},
        compat::FuturesAsyncWriteCompatExt,
    };
    let mut writer = FramedWrite::new(writer.compat_write(), LengthDelimitedCodec::new());
    let mut bytes = BytesMut::with_capacity(file_get_message.encoded_len());
    file_get_message.encode(&mut bytes).unwrap();
    writer.send(bytes.freeze()).await
}

async fn copy_stream<R, W>(reader: &mut R, writer: &mut W) -> Result<(), io::Error>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    #[cfg(feature = "tokio")]
    tokio_copy_stream(reader, writer).await
}

#[cfg(feature = "tokio")]
async fn tokio_copy_stream<R, W>(reader: &mut R, writer: &mut W) -> Result<(), io::Error>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    use tokio_util::compat::FuturesAsyncReadCompatExt;
    use tokio_util::compat::FuturesAsyncWriteCompatExt;

    tokio::io::copy(&mut reader.compat(), &mut writer.compat_write()).await?;
    Ok(())
}

async fn receive_message<R>(socket: R) -> Result<FileGetMessage, io::Error>
where
    R: AsyncRead + Unpin,
{
    #[cfg(feature = "tokio")]
    tokio_receive_message(socket).await
}

#[cfg(feature = "tokio")]
async fn tokio_receive_message<R>(socket: R) -> Result<FileGetMessage, io::Error>
where
    R: AsyncRead + Unpin,
{
    use tokio_util::{
        codec::{FramedRead, LengthDelimitedCodec},
        compat::FuturesAsyncReadCompatExt,
    };

    let mut reader = FramedRead::new(socket.compat(), LengthDelimitedCodec::new());
    if let Some(data) = reader.try_next().await? {
        let message = FileGetMessage::decode(data)
            .map_err(|_| io::Error::from(io::ErrorKind::UnexpectedEof))?;
        Ok(message)
    } else {
        Err(io::Error::from(io::ErrorKind::UnexpectedEof))
    }
}
