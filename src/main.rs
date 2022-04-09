use clap::Parser;

use rand::{thread_rng, Rng};
use rand::distributions::Alphanumeric;

use tokio::time::{self, Duration};
use tokio::sync::{mpsc, Mutex};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::WebSocketStream;

use std::sync::Arc;
use std::net::SocketAddr;
use std::collections::HashMap;

use futures_util::{SinkExt, StreamExt, stream::SplitSink};
use tokio_tungstenite::{accept_async, connect_async, tungstenite::Error};
use tungstenite::{Message, Result};

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
    /// Port on which a peer is created
    port: u16,
    #[clap(long)]
    /// Optional argument for secondary peers
    /// to connect to a primary one
    connect: Option<String>,
}

fn gen_message() -> String {
    thread_rng()
        .sample_iter(&Alphanumeric)
        .take(10)
        .map(char::from)
        .collect()
}

type PeerMap = Arc<Mutex<HashMap<u16, SplitSink<WebSocketStream<TcpStream>, Message>>>>;
struct PeerServer {
    server_port: u16,
    msg_interval: time::Interval,
    peer_map: PeerMap,
}

impl PeerServer {
    fn new(server_port: u16, msg_period: u64) -> Self {
        PeerServer {
            server_port,
            msg_interval: time::interval(Duration::from_secs(msg_period)),
            peer_map: Arc::new(Mutex::new(HashMap::new()))
        }
    }

}

async fn accept_connection(peer: SocketAddr, stream: TcpStream, msg_period: u64, server_port: u16, peer_map: PeerMap) {
    if let Err(e) = handle_connection(peer, stream, msg_period, server_port, peer_map).await {
        match e {
            Error::ConnectionClosed | Error::Protocol(_) | Error::Utf8 => (),
            err => log::error!("Error processing connection: {}", err),
        }
    }
}

async fn handle_connection(peer: SocketAddr, stream: TcpStream, msg_period: u64, server_port: u16, peer_map: PeerMap) -> Result<()> {
    let ws_stream = accept_async(stream).await.expect("Failed to accept");
    log::trace!("New WebSocket connection: {}", peer);
    let (ws_sender, mut ws_receiver) = ws_stream.split();
    peer_map.lock().await.insert(peer.port(), ws_sender);
    let mut interval = time::interval(Duration::from_secs(msg_period));
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
                            peer_map.lock().await.remove(&peer.port());
                            break;
                        }
                    }
                    None => break,
                }
            }
            _ = interval.tick() => {
                for (_, peer_sender) in peer_map.lock().await.iter_mut() {
                    peer_sender.send(Message::Text(format!("[{}]: {}", server_port, gen_message()))).await?;
                }
            },
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    pretty_env_logger::init_timed();

    let args = Args::parse();

    if let Some(parent) = args.connect {
        let connect_addr = format!("ws://127.0.0.1:{}", parent);
    
        let (ws_stream, _) = connect_async(connect_addr).await.expect("Failed to connect");
        log::trace!("WebSocket handshake has been successfully completed");
    
        let (mut client_ws_sender, mut client_ws_reader) = ws_stream.split();
    
        let mut interval = tokio::time::interval(Duration::from_secs(args.period));
    
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let msg = format!("[{}] says: {}", args.port, gen_message());
                    log::info!("Sending from {}", msg);
                    client_ws_sender.send(Message::Text(msg)).await?;
                },
                msg = client_ws_reader.next() => {
                    let data = msg.unwrap().unwrap().into_data();
                    log::info!("{}", std::str::from_utf8(&data[..])?);
                },
                _ = tokio::signal::ctrl_c() => {
                    log::info!("Client {} leaving chat", args.port);
                    client_ws_sender.send(Message::Close(None)).await?;
                    break;
                }
            }
        }
    } else {
        let addr = format!("127.0.0.1:{}", args.port);
        let listener = TcpListener::bind(&addr).await.expect("Can't listen");
        log::info!("Parent peer listening on: {}", addr);

        let peer_map = Arc::new(Mutex::new(HashMap::new()));
        while let Ok((stream, _)) = listener.accept().await {
            let peer = stream.peer_addr().expect("connected streams should have a peer address");
            log::info!("Peer address: {}", peer);
            tokio::spawn(accept_connection(peer, stream, args.period, args.port, peer_map.clone()));
        }
    }

    Ok(())
}