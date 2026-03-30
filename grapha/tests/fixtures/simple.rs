pub struct Config {
    pub debug: bool,
    pub name: String,
}

pub trait Configurable {
    fn configure(&self, config: &Config);
}

pub fn default_config() -> Config {
    Config {
        debug: false,
        name: String::new(),
    }
}
