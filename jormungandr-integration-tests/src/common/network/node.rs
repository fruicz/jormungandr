use crate::common::jormungandr::{JormungandrProcess, JormungandrRest};
pub struct Node {
    jormungandr: JormungandrProcess,
    alias: String,
}

impl Node {
    pub fn new(jormungandr: JormungandrProcess, alias: &str) -> Self {
        Self {
            jormungandr: jormungandr,
            alias: alias.to_string(),
        }
    }

    pub fn alias(&self) -> String {
        self.alias.to_string()
    }

    pub fn rest(&self) -> JormungandrRest {
        self.jormungandr.rest()
    }

    pub fn assert_no_errors_in_log(&self) {
        self.jormungandr.assert_no_errors_in_log();
    }

    pub fn public_id(&self) -> poldercast::Id {
        self.jormungandr.config.node_config.p2p.public_id.clone()
    }

    pub fn address(&self) -> poldercast::Address {
        self.jormungandr
            .config
            .node_config
            .p2p
            .public_address
            .clone()
    }

    pub fn shutdown(&self) {
        self.jormungandr.shutdown();
    }

    pub fn log_stats(&self) {
        println!("{}: {:?}", self.alias(), self.rest().stats().unwrap());
    }
}
