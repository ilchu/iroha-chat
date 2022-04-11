use anyhow::{anyhow, Result};
use clap::Parser;
use simplelog::{ColorChoice, Config, LevelFilter, TermLogger, TerminalMode};

use rand::distributions::Alphanumeric;
use rand::{thread_rng, Rng};

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::time::Duration;

use std::collections::HashMap;
use std::sync::Arc;

use tokio_tungstenite::{accept_async, connect_async, WebSocketStream};
use tungstenite::{Error, Message};

use futures_util::{stream::SplitSink, SinkExt, StreamExt};

#[derive(Parser, Debug)]
#[clap(author, version, about)]
/// Simple implementation of a p2p chat.
///
/// To start a new peer, pass `period` and
/// `port` required arguments. All following peers
/// should be connected to the first one via an
/// additional `connect` argument.
struct Args {
    #[clap(long)]
    /// Period in seconds with which a message will be sent
    period: u64,
    #[clap(long)]
    /// Port on which a peer is created (the actual port allocated by
    /// the server usually differs, thus this option only serves as a
    /// name)
    port: u16,
    #[clap(long)]
    /// Optional argument for secondary peers
    /// to connect to a primary one containing the parent
    /// port
    connect: Option<String>,
}

///
fn gen_message() -> String {
    thread_rng()
        .sample_iter(&Alphanumeric)
        .take(10)
        .map(char::from)
        .collect()
}

type PeerMap = Arc<Mutex<HashMap<u16, SplitSink<WebSocketStream<TcpStream>, Message>>>>;

async fn handle_connection(stream: TcpStream, peer_port: u16, peer_map: PeerMap) -> Result<()> {
    log::trace!("New WebSocket connection: {}", stream.local_addr()?);
    let ws_stream = accept_async(stream).await.expect("Failed to accept");
    let (ws_sender, mut ws_receiver) = ws_stream.split();
    peer_map.lock().await.insert(peer_port, ws_sender);
    log::trace!(
        "peer_map: {:?}",
        peer_map.lock().await.keys().collect::<Vec<_>>()
    );

    loop {
        tokio::select! {
            msg = ws_receiver.next() => {
                match msg {
                    Some(msg) => {
                        let msg = msg?;
                        if msg.is_text() || msg.is_binary() {
                            log::info!("{}", msg);
                            let text_msg = msg.to_text().unwrap();
                            let sender_port = text_msg.get(1..5).unwrap().parse::<u16>().unwrap();
                            for (peer_port, peer_sender) in peer_map.lock().await.iter_mut() {
                                if &sender_port != peer_port {
                                    peer_sender.send(msg.clone()).await?;
                                }
                            }
                        } else if msg.is_close() {
                            log::info!("[{}]: leaving chat", peer_port);
                            peer_map.lock().await.remove(&peer_port);
                            break;
                        }
                    }
                    None => {
                        log::trace!("Got none");
                        for (_, peer_sender) in peer_map.lock().await.iter_mut() {
                            peer_sender.send(Message::Close(None)).await?;
                        }
                        break;
                    },
                }
            },
        }
    }

    Ok(())
}
async fn accept_connection(stream: TcpStream, peer_port: u16, peer_map: PeerMap) -> Result<()> {
    if let Err(e) = handle_connection(stream, peer_port, peer_map).await {
        match e.downcast()? {
            Error::ConnectionClosed | Error::Protocol(_) | Error::Utf8 => Ok(()),
            err => Err(anyhow!("Connection could not be accepted: {}", err)),
        }
    } else {
        Ok(())
    }
}

async fn connect_client(
    connect_addr: String,
    client_port: u16,
    server_port: u16,
    msg_period: u64,
) -> Result<()> {
    let (ws_stream, _) = connect_async(connect_addr)
        .await
        .expect("Failed to connect");
    log::trace!("WebSocket handshake has been successfully completed");

    let (mut client_ws_sender, mut client_ws_reader) = ws_stream.split();
    let msg = format!("[{}]: joining chat", client_port);
    client_ws_sender.send(Message::Text(msg)).await?;

    let mut interval = tokio::time::interval(Duration::from_secs(msg_period));


    loop {
        tokio::select! {
            _ = interval.tick() => {
                let msg = format!("[{}] says: {}", client_port, gen_message());
                client_ws_sender.send(Message::Text(msg)).await?;
            },
            msg = client_ws_reader.next() => {
                if client_port != server_port {
                    match msg {
                        Some(msg) => {
                            let msg = msg?;
                            if msg.is_text() || msg.is_binary() {
                                log::info!("{}", msg);
                            } else if msg.is_close() {
                                log::info!("[{}]: leaving because of server...", client_port);
                                break;
                            }
                        },
                        None => {
                            log::info!("Got none");
                            break;
                        },
                    }
                }
            },
            _ = tokio::signal::ctrl_c() => {
                log::info!("[{}]: leaving chat", client_port);
                client_ws_sender.send(Message::Close(None)).await?;
                break;
            }
        }
    }
    Ok(())
}
#[tokio::main]
async fn main() -> Result<()> {
    TermLogger::init(
        LevelFilter::Info,
        Config::default(),
        TerminalMode::Stdout,
        ColorChoice::Auto,
    )?;

    let args = Args::parse();

    if let Some(parent) = args.connect {
        let connect_addr = format!("ws://127.0.0.1:{}", parent);
        let parent_port = parent.parse::<u16>().unwrap();

        connect_client(connect_addr, args.port, parent_port, args.period).await?;
    } else {
        let addr = format!("127.0.0.1:{}", args.port);
        let listener = TcpListener::bind(&addr).await.expect("Can't listen");
        log::trace!("Parent peer listening on: {}", addr);

        let peer_map: PeerMap = Arc::new(Mutex::new(HashMap::new()));

        let connect_addr = format!("ws://127.0.0.1:{}", args.port);
        tokio::spawn(connect_client(
            connect_addr,
            args.port,
            args.port,
            args.period,
        ));
        loop {
            tokio::select! {
                Ok((stream, _)) = listener.accept() => {
                    let peer = stream.peer_addr().expect("connected streams should have a peer address");
                    log::trace!("Peer address: {}", peer);
                    tokio::spawn(accept_connection(stream, peer.port(), peer_map.clone()));
                }
                _ = tokio::signal::ctrl_c() => {
                    for (_, peer_sender) in peer_map.lock().await.iter_mut() {
                        peer_sender.send(Message::Close(None)).await?;
                    }
                    break;
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_cmd::Command;
    #[test]
    fn test_fail_insufficient_args() {
        Command::cargo_bin("iroha-chat")
            .unwrap()
            .args(&["--port", "8080"])
            .timeout(Duration::from_secs(20))
            .assert()
            .failure();
    }
    #[test]
    fn test_time_period_one_client() -> Result<()> {
        let server = Command::cargo_bin("iroha-chat")
            .unwrap()
            .args(&["--port", "4000", "--period", "4"])
            .timeout(Duration::from_secs(30))
            .output()?;
        insta::assert_snapshot!(std::str::from_utf8(&server.stdout)?, @"");
        Ok(())
    }
    #[test]
    fn test_time_period_two_clients() -> Result<()> {
        let server = Command::cargo_bin("iroha-chat")
            .unwrap()
            .args(&["--port", "4000", "--period", "4"])
            .timeout(Duration::from_secs(30))
            .output()?;
        std::thread::sleep(Duration::from_millis(500));
        let client = Command::cargo_bin("iroha-chat")
            .unwrap()
            .args(&["--port", "5000", "--period", "5", "--connect", "4000"])
            .timeout(Duration::from_secs(30))
            .output()?;
        insta::assert_snapshot!(std::str::from_utf8(&server.stdout)?, @"");
        insta::assert_snapshot!(std::str::from_utf8(&client.stdout)?, @r###"
        09:36:36 [0m[34m[INFO] [0m[5000]: joining chat
        09:36:36 [0m[34m[INFO] [0m[5000] says: 0pSJzhpAbr
        09:36:39 [0m[34m[INFO] [0m[4000] says: ws0o7kyAuR
        09:36:41 [0m[34m[INFO] [0m[5000] says: O5YbfJEPTP
        09:36:43 [0m[34m[INFO] [0m[4000] says: 3j17pZdo8a
        09:36:46 [0m[34m[INFO] [0m[5000] says: FYIjjt367C
        09:36:47 [0m[34m[INFO] [0m[4000] says: A7aEfKvotd
        09:36:51 [0m[34m[INFO] [0m[5000] says: 9RkvEhGdzX
        09:36:51 [0m[34m[INFO] [0m[4000] says: 2YEAfeGlv4
        09:36:55 [0m[34m[INFO] [0m[4000] says: xyGPDleaJX
        09:36:56 [0m[34m[INFO] [0m[5000] says: D4Kgj9dlSG
        09:36:59 [0m[34m[INFO] [0m[4000] says: ioE9vWctUC
        09:37:01 [0m[34m[INFO] [0m[5000] says: Epf5dudGyV
        09:37:03 [0m[34m[INFO] [0m[4000] says: CvpncFc1rB
        "###);
        Ok(())
    }
}
