use actix_web::post;
use actix_web::{get, middleware::Logger, put, web, App, HttpResponse, HttpServer, Responder};
use actix_web_lab::sse;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use uuid::Uuid;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let maps = web::Data::new(Maps::new());

    HttpServer::new(move || {
        App::new()
            .wrap(Logger::default())
            .service(get_index)
            .service(create_upstream)
            .service(upstreams_events)
            .service(create_downstream)
            .service(subscribe_downstream)
            .service(post_upstream)
            .app_data(maps.clone())
    })
    .bind(("127.0.0.1", 8080))?
    .run()
    .await
}

struct Maps {
    upstreams: DashMap<Uuid, Upstream>,
    downstreams: DashMap<Uuid, Downstream>,
    upstreams_events: broadcast::Sender<UpstreamsEvent>,
}

impl Maps {
    fn new() -> Self {
        let (sender, _) = broadcast::channel(16);
        Self {
            upstreams: DashMap::new(),
            downstreams: DashMap::new(),
            upstreams_events: sender,
        }
    }
}

#[derive(Clone)]
struct Upstream {
    upstream_id: Uuid,
    message_sender: broadcast::Sender<String>,
}

impl Upstream {
    fn new() -> Self {
        Self {
            upstream_id: Uuid::new_v4(),
            message_sender: broadcast::channel(16).0,
        }
    }
}

struct Downstream {
    downstream_id: Uuid,
    message_receiver: Option<broadcast::Receiver<String>>,
    def: DownstreamDef,
}

impl Downstream {
    fn new(def: DownstreamDef) -> Self {
        Self {
            downstream_id: Uuid::new_v4(),
            message_receiver: None,
            def,
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
struct DownstreamDef {
    upstream_id: Option<Uuid>,
}

#[derive(Serialize, Clone)]
enum UpstreamsEvent {
    Created { upstream_id: Uuid },
}

#[put("/upstream")]
async fn create_upstream(maps: web::Data<Maps>) -> impl Responder {
    let upstream = Upstream::new();
    maps.upstreams
        .insert(upstream.upstream_id, upstream.clone());
    let _ = maps.upstreams_events.send(UpstreamsEvent::Created {
        upstream_id: upstream.upstream_id,
    });
    web::Json((upstream.upstream_id,))
}

#[derive(Serialize, Deserialize, Clone)]
struct UpstreamMessage {
    message: String,
}

#[post("/upstream/{upstream_id}")]
async fn post_upstream(
    path: web::Path<(Uuid,)>,
    maps: web::Data<Maps>,
    message: web::Json<UpstreamMessage>,
) -> impl Responder {
    if let Some(upstream) = maps.upstreams.get(&path.0) {
        let _ = upstream.message_sender.send(message.message.clone());
        Ok("")
    } else {
        Err(actix_web::error::ErrorNotFound("no upstream"))
    }
}

#[put("/downstream")]
async fn create_downstream(maps: web::Data<Maps>, def: web::Json<DownstreamDef>) -> impl Responder {
    let mut downstream = Downstream::new(def.0);
    if let Some(upstream_id) = downstream.def.upstream_id {
        if let Some(upstream) = maps.upstreams.get(&upstream_id) {
            let receiver = upstream.message_sender.subscribe();
            downstream.message_receiver = Some(receiver);
        }
    }
    let downstream_id = downstream.downstream_id;
    maps.downstreams.insert(downstream_id, downstream);
    println!("insert downstream {downstream_id}");
    web::Json((downstream_id,))
}

#[get("/downstream/{downstream_id}")]
async fn subscribe_downstream(path: web::Path<(Uuid,)>, maps: web::Data<Maps>) -> impl Responder {
    if let Some(downstream) = maps.downstreams.get(&path.0) {
        if let Some(ref receiver) = downstream.message_receiver {
            Ok(sse_from_broadcast_receiver(receiver.resubscribe()))
        } else {
            Err(actix_web::error::ErrorNotFound(
                "cant subscribe to downstream",
            ))
        }
    } else {
        Err(actix_web::error::ErrorNotFound("no downstream"))
    }
}

#[get("/upstreams")]
async fn upstreams_events(maps: web::Data<Maps>) -> impl Responder {
    let receiver = maps.upstreams_events.subscribe();
    sse_from_broadcast_receiver(receiver)
}

fn sse_from_broadcast_receiver<T: Sized + Serialize + Clone + Send + 'static>(
    receiver: broadcast::Receiver<T>,
) -> impl Responder {
    sse::Sse::from_stream(
        BroadcastStream::new(receiver)
            .filter_map(|it| it.ok().and_then(|it| sse::Data::new_json(it).ok()))
            .map(|data| Ok::<_, Infallible>(data.into())),
    )
}

#[get("/")]
async fn get_index() -> impl Responder {
    let html_file = std::fs::read_to_string("index.html").unwrap();
    HttpResponse::Ok().body(html_file)
}
