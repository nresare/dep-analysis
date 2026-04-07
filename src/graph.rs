// SPDX-License-Identifier: MIT

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::analysis::InternalDependency;

pub(crate) fn top_level_graph(dependencies: &[InternalDependency]) -> TopLevelGraph {
    let mut modules = BTreeSet::new();
    let mut edges = BTreeMap::<(String, String), usize>::new();

    for dependency in dependencies {
        let Some(from) = top_level_module(&dependency.from_module) else {
            continue;
        };
        let Some(to) = top_level_module(&dependency.to_module) else {
            continue;
        };
        if from == to {
            continue;
        }

        modules.insert(from.clone());
        modules.insert(to.clone());
        *edges.entry((from, to)).or_default() += 1;
    }

    TopLevelGraph {
        modules: modules.into_iter().collect(),
        edges: edges
            .into_iter()
            .map(|((from, to), count)| TopLevelEdge { from, to, count })
            .collect(),
    }
}

fn top_level_module(module: &str) -> Option<String> {
    let mut parts = module.split("::");
    if parts.next()? != "crate" {
        return None;
    }
    Some(parts.next()?.to_owned())
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct TopLevelGraph {
    pub(crate) modules: Vec<String>,
    pub(crate) edges: Vec<TopLevelEdge>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct TopLevelEdge {
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) count: usize,
}
