mod common;
mod config;
mod proxy;

use crate::config::Config;
use crate::proxy::*;

use std::collections::HashMap;
use uuid::Uuid;
use worker::*;
use once_cell::sync::Lazy;
use regex::Regex;

static PROXYIP_PATTERN: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(.+?)[:=-](\d{1,5})$").unwrap());
static PROXYKV_PATTERN: Lazy<Regex> = Lazy::new(|| Regex::new(r"^([a-zA-Z]{2})(,[a-zA-Z]{2})*$").unwrap());

#[event(fetch)]
async fn main(req: Request, env: Env, _: Context) -> Result<Response> {
    let uuid = env
        .var("UUID")
        .map(|x| Uuid::parse_str(&x.to_string()).unwrap_or_default())?;
    let host = req.url()?.host().map(|x| x.to_string()).unwrap_or_default();
    let main_page_url = env.var("MAIN_PAGE_URL").map(|x|x.to_string()).unwrap();
    let proxy_kv_url = env.var("PROXY_KV_URL").map(|x| x.to_string()).unwrap();
    let config = Config { uuid, proxy_addr: host, proxy_port: 443, main_page_url, proxy_kv_url };

    Router::with_data(config)
        .on_async("/", fe)
        .on_async("/free/cc/:proxyip", tunnel)
        .on_async("/free/:proxyip", tunnel)
        .on_async("/:proxyip", tunnel)
        .run(req, env)
        .await
}

async fn fe(_: Request, cx: RouteContext<Config>) -> Result<Response> {
    Response::redirect(cx.data.main_page_url.parse()?)
}

async fn tunnel(req: Request, mut cx: RouteContext<Config>) -> Result<Response> {
    let mut proxyip = cx.param("proxyip").unwrap().to_string();
    if PROXYKV_PATTERN.is_match(&proxyip)  {
        proxyip = proxyip.to_uppercase();
        let kvid_list: Vec<String> = proxyip.split(",").map(|s|s.to_string()).collect();
        let kv = cx.kv("SIREN")?;
        let mut proxy_kv_str = kv.get("proxy_kv").text().await?.unwrap_or("".to_string());
        let mut rand_buf = [0u8, 1];
        getrandom::getrandom(&mut rand_buf).expect("failed generating random number");
        
        if proxy_kv_str.len() == 0 {
            console_log!("getting proxy kv from github...");
            let req = Fetch::Url(Url::parse(&cx.data.proxy_kv_url)?);
            let mut res = req.send().await?;
            if res.status_code() == 200 {
                proxy_kv_str = res.text().await?.to_string();
                kv.put("proxy_kv", &proxy_kv_str)?.expiration_ttl(60 * 60 * 6).execute().await?; // 24 hours
            } else {
                return Err(Error::from(format!("error getting proxy kv: {}", res.status_code())));
            }
        }
        
        let proxy_kv: HashMap<String, Vec<String>> = serde_json::from_str(&proxy_kv_str)?;
        
        // select random KV ID
        let kv_index = (rand_buf[0] as usize) % kvid_list.len();
        proxyip = kvid_list[kv_index].clone();
        
        // select random proxy ip
        let proxyip_index = (rand_buf[0] as usize) % proxy_kv[&proxyip].len();
        proxyip = proxy_kv[&proxyip][proxyip_index].clone();
    }

    let upgrade = req.headers().get("Upgrade")?.unwrap_or_default();
    if upgrade == "websocket".to_string() && PROXYIP_PATTERN.is_match(&proxyip) {
        if let Some(captures) = PROXYIP_PATTERN.captures(&proxyip) {
            cx.data.proxy_addr = captures.get(1).unwrap().as_str().to_string();
            cx.data.proxy_port = captures.get(2).unwrap().as_str().parse()
                .map_err(|e| Error::from(format!("Invalid port number: {}", e)))?;
        }
        
        let WebSocketPair { server, client } = WebSocketPair::new()?;
        server.accept()?;
    
        wasm_bindgen_futures::spawn_local(async move {
            let events = server.events().unwrap();
            if let Err(e) = ProxyStream::new(cx.data, &server, events).process().await {
                console_error!("[tunnel]: {}", e);
            }
        });
    
        Response::from_websocket(client)
    } else {
        Response::redirect(cx.data.main_page_url.parse()?)
    }
}
