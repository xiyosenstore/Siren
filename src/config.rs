use uuid::Uuid;

pub struct Config {
    pub uuid: Uuid,
    pub proxy_addr: String,
    pub proxy_port: u16,

    pub main_page_url: String,
    pub proxy_kv_url: String,
}
