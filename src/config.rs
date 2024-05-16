use serde::Deserialize;
use tokio::io::AsyncReadExt;

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    admin: Vec<i64>,
    database: String,
    #[serde(default)]
    web: Web,
    platform: Upstream,
}

impl Config {
    pub async fn load(file: &str) -> anyhow::Result<Self> {
        let mut f = tokio::fs::File::open(file).await?;
        let mut s = String::new();

        f.read_to_string(&mut s).await?;
        Ok(toml::from_str(&s)?)
    }

    pub fn admin(&self) -> &Vec<i64> {
        &self.admin
    }

    pub fn platform(&self) -> &Upstream {
        &self.platform
    }

    pub fn web(&self) -> &Web {
        &self.web
    }

    pub fn database(&self) -> &str {
        &self.database
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Upstream {
    key: String,
    target: i64,
    server: Option<String>,
}

impl Upstream {
    pub fn server(&self) -> Option<&String> {
        self.server.as_ref()
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn target(&self) -> i64 {
        self.target
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Web {
    enabled: bool,
    port: u16,
    prefix: Option<String>,
    access_key: String,
}

impl Web {
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn prefix(&self) -> Option<&String> {
        self.prefix.as_ref()
    }

    pub fn access_key(&self) -> &str {
        &self.access_key
    }
}

impl Default for Web {
    fn default() -> Self {
        Self {
            enabled: false,
            port: 26511,
            prefix: None,
            access_key: "114514".to_string(),
        }
    }
}
