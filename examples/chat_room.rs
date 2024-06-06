use core::fmt;
use futures::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use std::{fmt::Debug, net::SocketAddr, sync::Arc};

use anyhow::Result;
use dashmap::DashMap;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::mpsc::{Receiver, Sender},
};
use tokio_util::codec::{Framed, LinesCodec};
use tracing::{info, level_filters::LevelFilter, warn};
use tracing_subscriber::{
    fmt::Layer, layer::SubscriberExt as _, util::SubscriberInitExt as _, Layer as _,
};

const MAX_MESSAGES: usize = 128;

#[derive(Debug)]
struct ChatRoom {
    peers: DashMap<SocketAddr, Sender<Arc<Message>>>,
}

#[derive(Debug)]
struct Peer {
    name: String,
    addr: SocketAddr,
    receiver: Receiver<Arc<Message>>,
}

#[derive(Debug, Clone)]
struct ChatMessage {
    from: String,
    content: String,
}

#[derive(Debug, Clone)]
enum Message {
    Join(String),
    Leave(String),
    Chat(ChatMessage),
}

#[tokio::main]
async fn main() -> Result<()> {
    let layer = Layer::new().with_filter(LevelFilter::INFO);
    tracing_subscriber::registry().with(layer).init();

    let addr = "0.0.0.0:4321";
    let listener = TcpListener::bind(addr).await?;
    info!("Listening on: {}", addr);

    let char_room = Arc::new(ChatRoom::new());

    loop {
        let (stream, addr) = listener.accept().await?;
        info!("Accepted connection from: {}", addr);

        let chat_room = char_room.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, addr, chat_room).await {
                warn!("handle client Error: {}", e);
            }
            info!("Connection from {} closed", addr);
        });
    }
}

async fn handle_client(
    stream: TcpStream,
    addr: SocketAddr,
    chat_room: Arc<ChatRoom>,
) -> Result<()> {
    let mut stream = Framed::new(stream, LinesCodec::new());

    stream.send("Please enter your name: ").await?;

    let name: String = match stream.next().await {
        Some(Ok(line)) => line,
        Some(Err(e)) => {
            return Err(e.into());
        }
        None => {
            return Ok(());
        }
    };

    stream.send(format!("Welcome! {}", name)).await?;

    let peer = chat_room.join(addr, name).await;

    peer.bootstrap(chat_room, stream).await?;

    Ok(())
}

impl Default for ChatRoom {
    fn default() -> Self {
        Self {
            peers: DashMap::new(),
        }
    }
}

impl ChatRoom {
    fn new() -> Self {
        Self::default()
    }

    async fn join(&self, addr: SocketAddr, name: String) -> Peer {
        let (tx, rx) = tokio::sync::mpsc::channel(MAX_MESSAGES);
        self.peers.insert(addr, tx);
        info!("{} joined the chat room", name);
        self.broadcast(addr, Arc::new(Message::join(&name))).await;
        Peer::new(addr, name, rx)
    }

    async fn leave(&self, addr: SocketAddr, name: String) {
        if self.peers.get(&addr).is_some() {
            self.peers.remove(&addr);
            info!("{} left the chat room", name);
            self.broadcast(addr, Arc::new(Message::leave(name))).await;
        }
    }

    async fn broadcast(&self, from: SocketAddr, message: Arc<Message>) {
        for item in self.peers.iter() {
            let key = item.key();
            if key == &from {
                continue;
            }

            let value = item.value();
            if let Err(e) = value.send(message.clone()).await {
                warn!("Failed to send message to peer {}: {}", key, e);
            }
        }
    }
}

impl Message {
    fn join(name: impl Into<String>) -> Self {
        Self::Join(name.into())
    }

    fn leave(name: impl Into<String>) -> Self {
        Self::Leave(name.into())
    }

    fn chat_message(from: impl Into<String>, content: impl Into<String>) -> Self {
        Self::Chat(ChatMessage {
            from: from.into(),
            content: content.into(),
        })
    }
}

impl Peer {
    fn new(addr: SocketAddr, name: String, receiver: Receiver<Arc<Message>>) -> Self {
        Self {
            addr,
            name,
            receiver,
        }
    }

    async fn bootstrap(
        self,
        chat_room: Arc<ChatRoom>,
        stream: Framed<TcpStream, LinesCodec>,
    ) -> Result<()> {
        let (sender, receiver) = stream.split();

        let name = self.name.clone();
        let addr = self.addr;
        let chat_room_cloned = chat_room.clone();
        tokio::spawn(async move {
            if let Err(e) = loop_receive_from_client(&name, addr, receiver, &chat_room_cloned).await
            {
                warn!(
                    "Failed to receive message from client, peer: {}, error: {}",
                    &name, e
                );
            }
            chat_room_cloned.leave(addr, name).await;
        });

        let name = self.name;
        if let Err(e) = loop_send_to_client(self.receiver, sender).await {
            warn!(
                "Failed to send message to client, peer: {}, error: {}",
                name, e
            );
        }
        chat_room.leave(addr, name).await;

        Ok(())
    }
}

async fn loop_send_to_client(
    mut rx: Receiver<Arc<Message>>,
    mut sender: SplitSink<Framed<TcpStream, LinesCodec>, String>,
) -> Result<()> {
    while let Some(message) = rx.recv().await {
        if let Err(e) = sender.send(message.to_string()).await {
            return Err(e.into());
        }
    }
    Ok(())
}

async fn loop_receive_from_client(
    name: &str,
    addr: SocketAddr,
    mut receiver: SplitStream<Framed<TcpStream, LinesCodec>>,
    chat_room: &Arc<ChatRoom>,
) -> Result<()> {
    while let Some(line) = receiver.next().await {
        let line = match line {
            Ok(line) => line,
            Err(e) => return Err(e.into()),
        };

        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let message = Arc::new(Message::chat_message(name, line));

        chat_room.broadcast(addr, message).await;
    }
    Ok(())
}

impl fmt::Display for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Join(name) => write!(f, "{} joined the chat room", name),
            Self::Leave(name) => write!(f, "{} left the chat room", name),
            Self::Chat(message) => write!(f, "{}: {}", message.from, message.content),
        }
    }
}
