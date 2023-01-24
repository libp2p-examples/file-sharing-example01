use clap::Parser;
use file_sharing_example::{Client, Command};
use futures::prelude::*;
use libp2p::Multiaddr;
use std::path::PathBuf;
use tokio_util::codec::FramedRead;

#[derive(Parser, Debug)]
#[clap(name = "rust-libp2p file sharing example")]
struct Cli {
    #[clap(long)]
    secret_key_seed: Option<u8>,

    #[clap(long)]
    sharing_dir: PathBuf,

    #[clap(long)]
    rendezvous_point_address: Multiaddr,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    let (mut client, event_loop) = file_sharing_example::new_network(
        cli.rendezvous_point_address,
        cli.secret_key_seed,
        cli.sharing_dir,
    )
    .await?;

    let mut stdin =
        FramedRead::new(tokio::io::stdin(), tokio_util::codec::LinesCodec::new()).fuse();

    tokio::spawn(event_loop.run());

    while let Some(line) = stdin.try_next().await? {
        if let Err(e) = handle_input(&mut client, line).await {
            println!("{e}");
        }
    }
    Ok(())
}

async fn handle_input(client: &mut Client, line: impl AsRef<str>) -> anyhow::Result<()> {
    let command = line.as_ref().parse::<Command>()?;
    client.send_command(command).await;

    Ok(())
}
