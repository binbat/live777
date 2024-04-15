use actix_web::{web, App, HttpResponse, HttpServer, Responder, middleware::Logger};
use std::sync::Arc;
use actix_cors::Cors;
use actix_files as fs;

use crate::config::Config;
use crate::storage_redis::{RedisStandaloneStorage, Node};
use crate::load_balancing::{LoadBalancing, RandomLoadBalancing,RoundRobinLoadBalancing};

mod config;
mod storage_redis;
mod load_balancing;


#[derive(Clone)]
struct AppState {
    config: Config,
    storage: Arc<RedisStandaloneStorage>,
    load_balancing: Arc<dyn LoadBalancing>,
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let config = Config::parse();
    let storage = Arc::new(RedisStandaloneStorage::new(&config.addr).await.unwrap());
    let nodes_result = storage.get_all_node().await;
    let nodes = match nodes_result {
    Ok(nodes) => nodes.into_iter().map(Arc::new).collect::<Vec<Arc<Node>>>(),
    Err(e) => {
        panic!("Failed to get nodes from Redis: {}", e);
        }
    };
    let load_balancer = match config.load_balancing.as_str() {
        "random" => Arc::new(RandomLoadBalancing::new(nodes)) as Arc<dyn LoadBalancing>,
        "localPolling" => Arc::new(RoundRobinLoadBalancing::new(nodes)) as Arc<dyn LoadBalancing>,
        _ => panic!("Unsupported load balancing strategy"),
    };
    let state = AppState { config,storage, load_balancing: load_balancer};
    let listen_addr = state.config.listen_addr.clone();
    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(state.clone()))
            .wrap(Logger::default())
            .wrap(Cors::permissive())
            .route("/whip/{room}", web::get().to(whip_handler))
            .route("/whep/{room}", web::get().to(proxy_handler))
            .route("/resource/{room}/{session}", web::get().to(proxy_handler))
            .route("/resource/{room}/{session}/layer", web::get().to(proxy_handler))
            .service(fs::Files::new("/", "./assets").index_file("index.html"))
    })
    .bind(&listen_addr)?
    .run()
    .await
}

async fn whip_handler(data: web::Data<AppState>, room: web::Path<String>) -> impl Responder {
    let storage = data.storage.clone();
    let room = room.into_inner();
    match storage.get_room_ownership(&room).await {
        Ok(Some(ownership)) => {
            HttpResponse::InternalServerError().body(format!("room has been used, node {}", ownership.addr))
        },
        Ok(None) => {
            match data.load_balancing.next().await {
                Ok(next) => {
                    proxy_request(&next.addr).await
                },
                Err(_) => HttpResponse::InternalServerError().body("Failed to get next node for load balancing"),
            }
        },
        Err(_) => HttpResponse::InternalServerError().body("Failed to get room ownership"),
    }
}

async fn proxy_handler(data: web::Data<AppState>, room: web::Path<String>) -> impl Responder {
    let storage = data.storage.clone();
    let room = room.into_inner();
    match storage.get_room_ownership(&room).await {
        Ok(Some(ownership)) => {
            proxy_request(&ownership.addr).await
        },
        Ok(None) => {
            HttpResponse::NotFound().body("The room does not exist")
        },
        Err(_) => HttpResponse::InternalServerError().body("Failed to get room ownership"),
    }
}

async fn proxy_request(node: &str) -> HttpResponse {
    let client = reqwest::Client::new();
    match client.get(&format!("http://{}", node)).send().await {
        Ok(resp) => {
            let content_type = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .map(|v| v.to_str().unwrap_or("application/octet-stream").to_string())
                .unwrap_or_else(|| "application/octet-stream".to_string());
            match resp.bytes().await {
                Ok(body) => {
                    HttpResponse::Ok()
                        .content_type(content_type) 
                        .body(body)
                },
                Err(_) => HttpResponse::InternalServerError().body("Failed to read proxy response"),
            }
        },
        Err(_) => HttpResponse::InternalServerError().body("Failed to proxy request"),
    }
}


