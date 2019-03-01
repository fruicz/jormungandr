//! module defining the p2p topology management objects
//!

use poldercast::topology::{Cyclon, Module, Rings, Topology, Vicinity};
pub use poldercast::{Address, Id, Node};
use poldercast::{InterestLevel, Proximity, Subscription, Subscriptions, Topic};
use std::collections::BTreeMap;

use std::sync::{Arc, RwLock};

/// object holding the P2pTopology of the Node
#[derive(Clone)]
pub struct P2pTopology {
    topology: Arc<RwLock<Topology>>,
}

impl P2pTopology {
    /// create a new P2pTopology for the given Address and Id
    ///
    /// The address is the public
    pub fn new(node: Node) -> Self {
        P2pTopology {
            topology: Arc::new(RwLock::new(Topology::new(node))),
        }
    }

    /// set a P2P Topology Module. Each module will work independently from
    /// each other and will help improve the node connectivity
    pub fn add_module<M: Module + Send + Sync + 'static>(&mut self, module: M) {
        info!("setting P2P Topology module: {}", module.name());
        self.topology.write().unwrap().add_module(module)
    }

    /// set all the default poldercast modules (Rings, Vicinity and Cyclon)
    pub fn set_poldercast_modules(&mut self) {
        let mut topology = self.topology.write().unwrap();
        topology.add_module(Rings::new());
        topology.add_module(Vicinity::new());
        topology.add_module(Cyclon::new());
    }

    /// this is the list of neighbors to contact for event dissemination
    pub fn view(&self) -> Vec<Node> {
        self.topology.read().unwrap().view()
    }

    /// this is the function to utilise when we receive a gossip in order
    /// to update the P2P Topology internal state
    pub fn update(&mut self, new_nodes: BTreeMap<Id, Node>) {
        self.topology.write().unwrap().update(new_nodes)
    }

    /// this is the function to utilise in order to select gossips to share
    /// with a given node
    pub fn select_gossips(&mut self, gossip_recipient: &Node) -> BTreeMap<Id, Node> {
        self.topology
            .write()
            .unwrap()
            .select_gossips(gossip_recipient)
    }
}