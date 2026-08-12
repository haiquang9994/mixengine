//! `depends_on` as a graph: what must be up before what, and in which order things come down.
//!
//! [`ServiceSpec::depends_on`] declares one service's edges; nothing until now has held the whole
//! set at once, and every interesting question needs the whole set. A cycle is the obvious one — no
//! single spec can see `a → b → a`, so [`mixengine_proto::ServiceSpecBuilder::build`] rejects only
//! the one case a spec *can* see, itself — but so are "what else has to start first" and "what
//! breaks if I stop this".
//!
//! **This is `core` and not `proto` or `supervisor`.** `proto` owns the vocabulary a spec is written
//! in ([ADR 0006](../../decisions/0006-servicespec-in-proto-and-secret-free.md)) and gains nothing
//! from a topological sort; the supervisor deliberately has no registry, no loop and no clock, which
//! is what lets T19 own the timing. What is left is domain logic over a set of declared services,
//! which is this crate.
//!
//! **A plan is tiers, not a list.** Two services that depend on nothing may start at the same time,
//! and a flat order throws that away and has to recompute it later. T19's runner walks the tiers one
//! service at a time to begin with; the day M3's ten-second budget wants concurrency, the
//! information is already here and nothing has to be re-derived. [`Plan::flat`] is the sequential
//! reading of the same value.
//!
//! Nothing here spawns, waits or persists. A graph is built from specs, answers questions about
//! them, and is thrown away — which is what makes every rule in it testable without a process.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use mixengine_proto::{ServiceId, ServiceSpec};

/// A set of service specs whose `depends_on` edges are known to form a DAG.
///
/// Built by [`ServiceGraph::new`], which is where the three things a single spec cannot check are
/// checked: that no id is declared twice, that every dependency named is a service that exists, and
/// that the edges do not close a loop. Past that point a graph answers questions and cannot fail on
/// its own account, so a caller holding one never has to handle "what if it is a cycle" again.
#[derive(Debug, Clone)]
pub struct ServiceGraph {
    /// Every service, by id. A `BTreeMap` rather than a `HashMap` because the order this iterates
    /// in is the order tiers come out in, and a start order that differs run to run turns a
    /// reproducible boot into an intermittent one.
    specs: BTreeMap<ServiceId, ServiceSpec>,

    /// The forward edges: for each service, what it names in its `depends_on`.
    ///
    /// A `BTreeSet` and not the declared `Vec`, which is the point of holding it at all: an edge
    /// declared twice has to *be* one edge. Counting `["mariadb", "mariadb"]` as two while the
    /// reverse map — a set — can only ever discharge one of them leaves a service waiting forever,
    /// which reads exactly like a loop and is not one. `ServiceSpecBuilder::build` refuses the
    /// repetition, but a spec deserialised from an `extension.toml` or a row has been through no
    /// builder, and this is the crate that trusts those edges.
    dependencies: BTreeMap<ServiceId, BTreeSet<ServiceId>>,

    /// The reverse edges: for each service, who names it in their `depends_on`.
    ///
    /// Stored rather than derived on demand because both directions are needed on every question
    /// worth asking — starting walks dependencies, stopping walks dependents — and computing this
    /// one from the other means a full scan of every spec each time.
    dependents: BTreeMap<ServiceId, BTreeSet<ServiceId>>,
}

impl ServiceGraph {
    /// Assemble a graph, checking everything the specs could not check individually.
    ///
    /// **This does not stand in for [`mixengine_proto::ServiceSpec::validate`]**, which is a
    /// loader's job and reports against the row or file a bad spec came from. It does not depend on
    /// it either: a spec that never went through one is still assembled into edges that mean what
    /// they say, so a graph is never the thing that mistakes an unvalidated spec for a broken one.
    ///
    /// # Errors
    ///
    /// [`GraphError::Duplicate`] when two specs carry the same id; [`GraphError::UnknownDependency`]
    /// when a spec names a dependency that is not in the set; [`GraphError::Cycle`] when the edges
    /// close a loop, carrying the loop itself rather than the fact of one.
    pub fn new(specs: impl IntoIterator<Item = ServiceSpec>) -> Result<Self, GraphError> {
        let mut by_id: BTreeMap<ServiceId, ServiceSpec> = BTreeMap::new();

        for spec in specs {
            if let Some(existing) = by_id.insert(spec.id().clone(), spec) {
                return Err(GraphError::Duplicate {
                    id: existing.id().clone(),
                });
            }
        }

        // Seeded with an entry per service, so `dependents` is total over the graph's ids and every
        // lookup below is an ordinary `get` rather than a question about whether an id is known.
        let mut dependents: BTreeMap<ServiceId, BTreeSet<ServiceId>> = by_id
            .keys()
            .map(|id| (id.clone(), BTreeSet::new()))
            .collect();
        let mut dependencies: BTreeMap<ServiceId, BTreeSet<ServiceId>> = BTreeMap::new();

        // Every edge is checked before any is walked, so an unknown dependency is reported as
        // itself rather than as a service that mysteriously never gets a tier. Both directions are
        // built here and both are sets, so an edge declared twice is one edge in each.
        for spec in by_id.values() {
            let mut declared: BTreeSet<ServiceId> = BTreeSet::new();

            for dependency in spec.depends_on() {
                let Some(dependents) = dependents.get_mut(dependency) else {
                    return Err(GraphError::UnknownDependency {
                        service: spec.id().clone(),
                        dependency: dependency.clone(),
                    });
                };

                dependents.insert(spec.id().clone());
                declared.insert(dependency.clone());
            }

            dependencies.insert(spec.id().clone(), declared);
        }

        let graph = Self {
            specs: by_id,
            dependencies,
            dependents,
        };

        // Kahn's algorithm consumes exactly the acyclic part of a graph, so anything it leaves
        // behind is in a loop or downstream of one. `find_cycle` pays a second walk — only on the
        // failing path — to say which services those are.
        let everything: BTreeSet<ServiceId> = graph.specs.keys().cloned().collect();
        if graph.layered(&everything, Direction::Dependencies).is_err() {
            return Err(GraphError::Cycle {
                path: graph.find_cycle(),
            });
        }

        Ok(graph)
    }

    /// The spec for one service, or `None` if it is not in this graph.
    #[must_use]
    pub fn spec(&self, id: &ServiceId) -> Option<&ServiceSpec> {
        self.specs.get(id)
    }

    /// How many services the graph holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.specs.len()
    }

    /// Whether the graph holds no services at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }

    /// Every service, in an order where nothing is reached before what it depends on.
    ///
    /// What autostart at boot walks.
    #[must_use]
    pub fn start_order(&self) -> Plan {
        self.plan(
            &self.specs.keys().cloned().collect(),
            Direction::Dependencies,
        )
    }

    /// Every service, in an order where nothing is reached before what depends on it.
    ///
    /// What `daemon.shutdown` walks (T9a). **Not [`ServiceGraph::start_order`] reversed** — for the
    /// whole set the two do coincide, but they are computed from opposite edges and only
    /// [`ServiceGraph::stop_plan`] gives the right answer for a subset, so deriving one from the
    /// other here would be a coincidence waiting to be relied on.
    #[must_use]
    pub fn stop_order(&self) -> Plan {
        self.plan(&self.specs.keys().cloned().collect(), Direction::Dependents)
    }

    /// What must start, and in what order, for `roots` to be running.
    ///
    /// The plan holds the roots **and everything they transitively depend on**: `mix service start
    /// php-fpm@8.3` on a service that names MariaDB is a request to have MariaDB up, and refusing it
    /// would be technically correct and useless.
    ///
    /// # Errors
    ///
    /// [`GraphError::NoSuchService`] when a root is not in this graph.
    pub fn start_plan<'a>(
        &self,
        roots: impl IntoIterator<Item = &'a ServiceId>,
    ) -> Result<Plan, GraphError> {
        Ok(self.plan(&self.known(roots)?, Direction::Dependencies))
    }

    /// What must stop, and in what order, for `roots` to be stopped.
    ///
    /// The mirror image, and the asymmetry is the point: stopping pulls in everything that
    /// transitively **depends on** the roots, because a site left pointed at a database that is
    /// going away is worse than one told its database is down. They come first in the plan, so no
    /// service is ever left running without what it needs.
    ///
    /// # Errors
    ///
    /// [`GraphError::NoSuchService`] when a root is not in this graph.
    pub fn stop_plan<'a>(
        &self,
        roots: impl IntoIterator<Item = &'a ServiceId>,
    ) -> Result<Plan, GraphError> {
        Ok(self.plan(&self.known(roots)?, Direction::Dependents))
    }

    /// The services `id` names directly in its `depends_on`.
    ///
    /// The graph's edges rather than the spec's list — each dependency once, in [`ServiceId`] order,
    /// the same shape [`ServiceGraph::dependents_of`] answers in. Read the spec for what was
    /// literally written.
    ///
    /// # Errors
    ///
    /// [`GraphError::NoSuchService`] when `id` is not in this graph.
    pub fn dependencies_of(&self, id: &ServiceId) -> Result<&BTreeSet<ServiceId>, GraphError> {
        self.dependencies
            .get(id)
            .ok_or_else(|| GraphError::NoSuchService { id: id.clone() })
    }

    /// The services that name `id` directly in their `depends_on`.
    ///
    /// # Errors
    ///
    /// [`GraphError::NoSuchService`] when `id` is not in this graph.
    pub fn dependents_of(&self, id: &ServiceId) -> Result<&BTreeSet<ServiceId>, GraphError> {
        self.dependents
            .get(id)
            .ok_or_else(|| GraphError::NoSuchService { id: id.clone() })
    }

    /// Everything that transitively depends on `id`, `id` itself excluded.
    ///
    /// **What a failure propagates along.** T19's runner asks this when a service fails to start:
    /// every planned service in the answer is one that can now never come up, and goes straight to
    /// [`mixengine_proto::ServiceState::Failed`] with
    /// [`mixengine_proto::StateReason::DependencyFailed`] instead of being spawned against a
    /// dependency that is not there. Ordered, so the same failure reads the same way twice.
    ///
    /// # Errors
    ///
    /// [`GraphError::NoSuchService`] when `id` is not in this graph.
    pub fn blocked_by(&self, id: &ServiceId) -> Result<BTreeSet<ServiceId>, GraphError> {
        let roots = self.known(std::iter::once(id))?;
        let mut blocked = self.reachable(&roots, Direction::Dependents);
        blocked.remove(id);

        Ok(blocked)
    }

    /// Check that every root is a service this graph holds, and collect them.
    fn known<'a>(
        &self,
        roots: impl IntoIterator<Item = &'a ServiceId>,
    ) -> Result<BTreeSet<ServiceId>, GraphError> {
        roots
            .into_iter()
            .map(|id| {
                if self.specs.contains_key(id) {
                    Ok(id.clone())
                } else {
                    Err(GraphError::NoSuchService { id: id.clone() })
                }
            })
            .collect()
    }

    /// A plan over the sub-graph reachable from `roots`, which must already be known ids.
    ///
    /// The layering cannot fail here: a `ServiceGraph` is acyclic by construction, and a sub-graph
    /// of a DAG is a DAG. [`ServiceGraph::new`] is the one caller that has neither guarantee yet,
    /// and it calls [`ServiceGraph::layered`] directly for exactly that reason.
    fn plan(&self, roots: &BTreeSet<ServiceId>, direction: Direction) -> Plan {
        let included = self.reachable(roots, direction);
        let tiers = self
            .layered(&included, direction)
            .expect("a ServiceGraph is acyclic by construction");

        Plan { tiers }
    }

    /// The roots plus everything reachable from them along `direction`.
    fn reachable(&self, roots: &BTreeSet<ServiceId>, direction: Direction) -> BTreeSet<ServiceId> {
        let mut seen: BTreeSet<ServiceId> = roots.clone();
        let mut queue: VecDeque<ServiceId> = roots.iter().cloned().collect();

        while let Some(id) = queue.pop_front() {
            for next in self.edges(&id, direction) {
                if seen.insert(next.clone()) {
                    queue.push_back(next.clone());
                }
            }
        }

        seen
    }

    /// Kahn's algorithm over `included`, emitting one tier per round.
    ///
    /// Taking *every* currently-free service per round rather than one at a time is what produces
    /// the tiers: a service is free once everything it must wait for has been emitted, so the
    /// members of a round have no path between them and cannot constrain each other's order.
    ///
    /// The `direction` is what "wait for" means. Starting waits for dependencies, so tier 0 is the
    /// services that depend on nothing; stopping waits for dependents, so tier 0 is the services
    /// nothing depends on. Both are the same walk with the edges read the other way round.
    ///
    /// # Errors
    ///
    /// [`HasCycle`] when a round comes up empty with services still unemitted — the definition of a
    /// loop, since every one of them is still waiting for another one of them.
    fn layered(
        &self,
        included: &BTreeSet<ServiceId>,
        direction: Direction,
    ) -> Result<Vec<Vec<ServiceId>>, HasCycle> {
        // How many services inside the sub-graph each one is still waiting for.
        let mut waiting_on: BTreeMap<ServiceId, usize> = included
            .iter()
            .map(|id| {
                let count = self
                    .edges(id, direction)
                    .iter()
                    .filter(|next| included.contains(*next))
                    .count();
                (id.clone(), count)
            })
            .collect();

        let mut tiers: Vec<Vec<ServiceId>> = Vec::new();
        let mut emitted = 0_usize;

        loop {
            let tier: Vec<ServiceId> = waiting_on
                .iter()
                .filter(|(_, waiting)| **waiting == 0)
                .map(|(id, _)| id.clone())
                .collect();

            if tier.is_empty() {
                break;
            }

            for id in &tier {
                waiting_on.remove(id);

                // Whoever was waiting on this one is now one step freer, and the edges run the
                // other way to find them: a released dependency frees its dependents, and a
                // released dependent frees what it was holding up. Exactly one decrement per edge
                // counted above, because both directions are sets over the same edges.
                for freed in self.edges(id, direction.reversed()) {
                    if let Some(waiting) = waiting_on.get_mut(freed) {
                        *waiting -= 1;
                    }
                }
            }

            emitted += tier.len();
            tiers.push(tier);
        }

        if emitted == included.len() {
            Ok(tiers)
        } else {
            Err(HasCycle)
        }
    }

    /// One service's neighbours along `direction`, each named once.
    ///
    /// Both maps are total over the graph's ids, so an id from outside simply has no neighbours
    /// rather than being a case every caller has to answer for.
    fn edges(&self, id: &ServiceId, direction: Direction) -> &BTreeSet<ServiceId> {
        /// The answer for an id this graph does not hold.
        static NONE: BTreeSet<ServiceId> = BTreeSet::new();

        let map = match direction {
            Direction::Dependencies => &self.dependencies,
            Direction::Dependents => &self.dependents,
        };

        map.get(id).unwrap_or(&NONE)
    }

    /// Recover one concrete loop, for the error message.
    ///
    /// Run only once [`ServiceGraph::layered`] has proved there is one, because it is a second walk
    /// and the answer is worth it exactly once: "there is a loop among caddy, php-fpm and mariadb"
    /// leaves the user to work out which edge to delete, while `caddy → php-fpm → mariadb → caddy`
    /// names it.
    ///
    /// A depth-first search that keeps its own stack rather than recursing, so a chain longer than
    /// the machine's stack is an error message rather than a crash — these specs can come from an
    /// `extension.toml`.
    fn find_cycle(&self) -> Vec<ServiceId> {
        /// Where the search has got to with a service.
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Visit {
            /// On the current path: reaching it again closes a loop.
            Open,
            /// Fully explored, and led to no loop.
            Closed,
        }

        let mut seen: BTreeMap<&ServiceId, Visit> = BTreeMap::new();

        for root in self.specs.keys() {
            if seen.contains_key(root) {
                continue;
            }

            // The path so far, each step carrying the dependencies it has not tried yet.
            let mut path: Vec<(&ServiceId, Declared<'_>)> = Vec::new();
            seen.insert(root, Visit::Open);
            path.push((root, self.declared(root)));

            while let Some((id, remaining)) = path.last_mut() {
                let here = *id;

                match remaining.next() {
                    Some(next) => match seen.get(next) {
                        // `next` is somewhere on the current path, so the loop runs from there to
                        // here. Every step between the two is in it, and nothing before.
                        Some(Visit::Open) => {
                            let start = path
                                .iter()
                                .position(|(id, _)| *id == next)
                                .unwrap_or_default();

                            return path[start..].iter().map(|(id, _)| (*id).clone()).collect();
                        }
                        Some(Visit::Closed) => {}
                        None => {
                            seen.insert(next, Visit::Open);
                            path.push((next, self.declared(next)));
                        }
                    },
                    None => {
                        seen.insert(here, Visit::Closed);
                        path.pop();
                    }
                }
            }
        }

        // Not reachable through `new`: `layered` leaves a service unconsumed only when it is
        // waiting on one that is never emitted, and with both edge directions held as sets over the
        // same edges the only way that happens is a real loop. An empty path renders as a loop with
        // no names rather than as a panic, because a thin error message is a better failure than a
        // dead daemon.
        Vec::new()
    }

    /// A service's declared dependencies, as the borrowing iterator [`ServiceGraph::find_cycle`]
    /// parks on its stack.
    fn declared(&self, id: &ServiceId) -> Declared<'_> {
        self.edges(id, Direction::Dependencies).iter()
    }
}

/// One service's dependencies, part-way through being walked.
type Declared<'a> = std::collections::btree_set::Iter<'a, ServiceId>;

/// Which way an edge is being read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    /// Towards what a service needs: `depends_on`. Starting walks this way.
    Dependencies,
    /// Towards what needs a service. Stopping walks this way.
    Dependents,
}

impl Direction {
    /// The other way, which is how a freed service finds who was waiting on it.
    const fn reversed(self) -> Self {
        match self {
            Self::Dependencies => Self::Dependents,
            Self::Dependents => Self::Dependencies,
        }
    }
}

/// "There is a loop", before [`ServiceGraph::find_cycle`] has named it.
///
/// Private and deliberately empty: the only caller that can meet it is [`ServiceGraph::new`], which
/// turns it into a [`GraphError::Cycle`] that carries the loop. Everywhere else a graph is already
/// acyclic, so this is the signal that a `Result` is being handled rather than a failure a user ever
/// reads.
#[derive(Debug, PartialEq, Eq)]
struct HasCycle;

/// An ordered set of services to act on: tier by tier, with everything a tier needs already done by
/// the time it is reached.
///
/// **The tiers are kept rather than flattened** because they carry the one thing a flat list
/// destroys: which services have no ordering constraint between them. T19 walks a plan sequentially
/// through [`Plan::flat`]; the concurrency M3's ten-second budget will eventually want is then a
/// change to the walker, not a recomputation of the plan.
///
/// Within a tier the order is by [`ServiceId`], so the same specs always produce the same plan — a
/// boot order that varies run to run turns one broken dependency into a bug that only reproduces on
/// somebody else's machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    tiers: Vec<Vec<ServiceId>>,
}

impl Plan {
    /// The tiers, in the order they must be acted on. Everything within one may proceed at once.
    #[must_use]
    pub fn tiers(&self) -> &[Vec<ServiceId>] {
        &self.tiers
    }

    /// Every service in the plan, flattened into one valid sequential order.
    pub fn flat(&self) -> impl Iterator<Item = &ServiceId> {
        self.tiers.iter().flatten()
    }

    /// How many services the plan covers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tiers.iter().map(Vec::len).sum()
    }

    /// Whether the plan asks for nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tiers.is_empty()
    }

    /// Whether a service is one of the plan's.
    #[must_use]
    pub fn contains(&self, id: &ServiceId) -> bool {
        self.flat().any(|planned| planned == id)
    }
}

/// A set of specs that cannot be assembled into a dependency graph, or a question asked of one about
/// a service it does not hold.
///
/// Its own type rather than more variants on [`crate::Error`], and reachable from there through
/// [`crate::Error::Graph`]: these are failures of *declaring* services, they name no path and no
/// database, and the daemon reports them against whatever produced the specs — a package definition,
/// an `extension.toml` — rather than against the machine.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum GraphError {
    /// Two specs claim the same id.
    ///
    /// Not merged: one of them would silently win, and which one would depend on the order they
    /// happened to be loaded in.
    #[error("service `{id}` is declared more than once")]
    Duplicate {
        /// The id claimed twice.
        id: ServiceId,
    },

    /// A spec depends on a service that is not in the set.
    ///
    /// Almost always a typo, or a service deleted while something still named it, and it is refused
    /// rather than ignored: a dependency treated as satisfied because nobody can find it is the
    /// ordering bug that only appears on a slow machine.
    #[error("service `{service}` depends on `{dependency}`, which is not a service here")]
    UnknownDependency {
        /// The spec that declared the edge.
        service: ServiceId,
        /// The dependency it named.
        dependency: ServiceId,
    },

    /// The edges close a loop, so no service in it could ever be started first.
    #[error(
        "these services depend on each other in a loop: {}",
        render_cycle(path)
    )]
    Cycle {
        /// The loop, each service once and in order. The last one depends on the first, which is
        /// what closes it — the repetition is in the rendering, not in the data.
        path: Vec<ServiceId>,
    },

    /// A question was asked about a service the graph does not hold.
    #[error("no such service: `{id}`")]
    NoSuchService {
        /// The id that was asked about.
        id: ServiceId,
    },
}

/// `caddy → php-fpm@8.3 → mariadb@main → caddy`: the loop written out, closed back to its start.
fn render_cycle(path: &[ServiceId]) -> String {
    let Some(first) = path.first() else {
        return "(the loop could not be recovered)".to_owned();
    };

    let mut rendered = String::new();
    for id in path {
        rendered.push_str(id.as_str());
        rendered.push_str(" → ");
    }
    rendered.push_str(first.as_str());

    rendered
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use mixengine_proto::{Millis, ReadyCheck};

    use super::*;

    fn id(value: &str) -> ServiceId {
        ServiceId::parse(value).expect("a valid service id")
    }

    /// A spec that describes nothing interesting, so a test can be about its edges and nothing else.
    fn spec(name: &str, depends_on: &[&str]) -> ServiceSpec {
        let root: PathBuf = if cfg!(windows) {
            r"C:\MixEngine".into()
        } else {
            "/opt/mixengine".into()
        };

        let mut builder = ServiceSpec::builder(id(name), root.join(name))
            .cwd(root)
            .ready(ReadyCheck::PidAlive {
                settle: Millis::from_secs(1),
            });

        for dependency in depends_on {
            builder = builder.depends_on(id(dependency));
        }

        builder.build().expect("a usable spec")
    }

    /// A spec carrying edges [`mixengine_proto::ServiceSpecBuilder::build`] would refuse.
    ///
    /// Not a contrived shape: `ServiceSpec`'s own documentation says deserialisation runs no checks
    /// and leaves them to whoever loads it, so this is exactly what a `services` row or an
    /// `extension.toml` hands the graph when nobody called `validate` on the way.
    fn unchecked(name: &str, depends_on: &[&str]) -> ServiceSpec {
        let edges = depends_on
            .iter()
            .map(|id| format!(r#""{id}""#))
            .collect::<Vec<_>>()
            .join(",");

        let encoded = serde_json::to_string(&spec(name, &[])).expect("a spec serialises");
        serde_json::from_str(
            &encoded.replace(r#""depends_on":[]"#, &format!(r#""depends_on":[{edges}]"#)),
        )
        .expect("deserialisation deliberately does not validate")
    }

    fn graph(specs: &[(&str, &[&str])]) -> ServiceGraph {
        ServiceGraph::new(
            specs
                .iter()
                .map(|(name, depends_on)| spec(name, depends_on)),
        )
        .expect("an acyclic graph")
    }

    /// `["caddy", "mariadb"]` reads better in an assertion than a `Vec<ServiceId>` does.
    fn names<'a>(ids: impl IntoIterator<Item = &'a ServiceId>) -> Vec<&'a str> {
        ids.into_iter().map(ServiceId::as_str).collect()
    }

    fn tiers(plan: &Plan) -> Vec<Vec<&str>> {
        plan.tiers().iter().map(names).collect()
    }

    #[test]
    fn a_dependency_is_in_an_earlier_tier_than_what_needs_it() {
        let graph = graph(&[
            ("caddy", &[]),
            ("mariadb", &[]),
            ("php-fpm", &["mariadb", "redis"]),
            ("redis", &[]),
        ]);

        assert_eq!(
            tiers(&graph.start_order()),
            vec![vec!["caddy", "mariadb", "redis"], vec!["php-fpm"]],
            "the three that wait for nothing share a tier; the one that waits comes after"
        );
    }

    /// The property the whole module exists for, asserted as a property rather than as one expected
    /// list: a plan is valid exactly when nothing appears in it before something it needs.
    #[test]
    fn nothing_is_ever_planned_before_what_it_depends_on() {
        let graph = graph(&[
            ("caddy", &["php-fpm"]),
            ("php-fpm", &["mariadb", "redis"]),
            ("mariadb", &[]),
            ("redis", &["mariadb"]),
            ("mailpit", &[]),
        ]);

        let plan = graph.start_order();
        let order = names(plan.flat());

        for (position, service) in order.iter().enumerate() {
            for dependency in graph
                .dependencies_of(&id(service))
                .expect("a service of this graph")
            {
                let at = order
                    .iter()
                    .position(|planned| *planned == dependency.as_str())
                    .expect("a full plan holds every dependency");

                assert!(at < position, "{service} was planned before {dependency}");
            }
        }
    }

    #[test]
    fn stopping_is_the_other_way_round() {
        let graph = graph(&[
            ("caddy", &["php-fpm"]),
            ("php-fpm", &["mariadb"]),
            ("mariadb", &[]),
        ]);

        assert_eq!(
            names(graph.stop_order().flat()),
            vec!["caddy", "php-fpm", "mariadb"],
            "the database is the last thing to go"
        );
    }

    /// Starting one service is a request to have what it needs, so the plan grows downwards.
    #[test]
    fn a_start_plan_pulls_in_what_the_roots_need() {
        let graph = graph(&[
            ("caddy", &["php-fpm"]),
            ("php-fpm", &["mariadb"]),
            ("mariadb", &[]),
            ("mailpit", &[]),
        ]);

        let plan = graph
            .start_plan(std::iter::once(&id("caddy")))
            .expect("caddy is in the graph");

        assert_eq!(names(plan.flat()), vec!["mariadb", "php-fpm", "caddy"]);
        assert!(
            !plan.contains(&id("mailpit")),
            "a service nothing asked for and nothing needs is not started"
        );
    }

    /// And stopping one is a decision about everything that would be left pointing at it, so the
    /// same graph plans the other way.
    #[test]
    fn a_stop_plan_pulls_in_what_needs_the_roots() {
        let graph = graph(&[
            ("caddy", &["php-fpm"]),
            ("php-fpm", &["mariadb"]),
            ("mariadb", &[]),
            ("mailpit", &[]),
        ]);

        let plan = graph
            .stop_plan(std::iter::once(&id("mariadb")))
            .expect("mariadb is in the graph");

        assert_eq!(
            names(plan.flat()),
            vec!["caddy", "php-fpm", "mariadb"],
            "what depends on the database goes down before the database does"
        );
        assert!(!plan.contains(&id("mailpit")));
    }

    /// Both directions are public, answer in the same shape, and name the service rather than
    /// shrugging when it is not here — T19 reads one to spawn and the other to give up.
    #[test]
    fn a_service_knows_both_what_it_needs_and_what_needs_it() {
        let graph = graph(&[
            ("caddy", &["php-fpm"]),
            ("php-fpm", &["mariadb"]),
            ("mariadb", &[]),
        ]);

        assert_eq!(
            names(
                graph
                    .dependencies_of(&id("php-fpm"))
                    .expect("a known service")
            ),
            vec!["mariadb"]
        );
        assert_eq!(
            names(
                graph
                    .dependents_of(&id("php-fpm"))
                    .expect("a known service")
            ),
            vec!["caddy"]
        );
        assert!(
            graph
                .dependencies_of(&id("mariadb"))
                .expect("a known service")
                .is_empty(),
            "the bottom of the graph needs nothing"
        );
        assert!(
            graph
                .dependents_of(&id("caddy"))
                .expect("a known service")
                .is_empty(),
            "and the top holds nothing up"
        );

        for outcome in [
            graph.dependencies_of(&id("mailpit")),
            graph.dependents_of(&id("mailpit")),
        ] {
            assert_eq!(
                outcome.expect_err("mailpit is not in this graph"),
                GraphError::NoSuchService { id: id("mailpit") }
            );
        }
    }

    /// What T19 spawns from: the plan names a service, and the graph is where its spec comes from.
    #[test]
    fn the_spec_that_went_in_is_the_spec_that_comes_back() {
        let graph = graph(&[("caddy", &[]), ("mariadb", &[])]);

        assert_eq!(graph.spec(&id("caddy")), Some(&spec("caddy", &[])));
        assert_eq!(graph.spec(&id("mailpit")), None);
    }

    #[test]
    fn a_plan_for_nothing_asks_for_nothing() {
        let graph = graph(&[("caddy", &[]), ("mariadb", &[])]);

        let plan = graph.start_plan(std::iter::empty()).expect("no roots");

        assert!(plan.is_empty());
        assert_eq!(plan.len(), 0);
    }

    #[test]
    fn a_root_that_is_not_a_service_is_named() {
        let graph = graph(&[("caddy", &[])]);

        let error = graph
            .start_plan(std::iter::once(&id("mariadb")))
            .expect_err("no such service");

        assert_eq!(error, GraphError::NoSuchService { id: id("mariadb") });
    }

    /// What T19 asks after a service fails to start: everything that can now never come up.
    #[test]
    fn a_failure_blocks_everything_downstream_of_it() {
        let graph = graph(&[
            ("caddy", &["php-fpm"]),
            ("php-fpm", &["mariadb"]),
            ("mariadb", &[]),
            ("redis", &[]),
        ]);

        let blocked = graph.blocked_by(&id("mariadb")).expect("a known service");

        assert_eq!(
            names(&blocked),
            vec!["caddy", "php-fpm"],
            "the whole chain, not only the service that named it"
        );
        assert!(
            graph
                .blocked_by(&id("redis"))
                .expect("a known service")
                .is_empty(),
            "nothing declared a dependency on redis"
        );
    }

    #[test]
    fn a_loop_is_refused_and_written_out() {
        let error = ServiceGraph::new([
            spec("caddy", &["php-fpm"]),
            spec("php-fpm", &["mariadb"]),
            spec("mariadb", &["caddy"]),
        ])
        .expect_err("a loop is not a graph");

        let GraphError::Cycle { path } = &error else {
            panic!("{error:?}");
        };

        assert_eq!(path.len(), 3, "each service once, closed by the rendering");
        assert_eq!(
            error.to_string(),
            "these services depend on each other in a loop: caddy → php-fpm → mariadb → caddy"
        );
    }

    /// The tail of a chain can be a loop without the head being in it, and the message has to name
    /// the loop rather than the walk that found it.
    #[test]
    fn a_loop_reached_through_a_chain_names_only_the_loop() {
        let error = ServiceGraph::new([
            spec("caddy", &["php-fpm"]),
            spec("php-fpm", &["mariadb"]),
            spec("mariadb", &["redis"]),
            spec("redis", &["mariadb"]),
        ])
        .expect_err("a loop is not a graph");

        let GraphError::Cycle { path } = &error else {
            panic!("{error:?}");
        };

        assert_eq!(
            names(path),
            vec!["mariadb", "redis"],
            "caddy and php-fpm lead to the loop but are not in it"
        );
    }

    /// A spec cannot be *built* depending on itself, so the only way to hold one is the way a
    /// `services` row or an `extension.toml` arrives: deserialised, where nothing is checked.
    #[test]
    fn a_service_that_depends_on_itself_is_a_loop_of_one() {
        let error = ServiceGraph::new([unchecked("caddy", &["caddy"])]).expect_err("a loop of one");

        assert_eq!(
            error,
            GraphError::Cycle {
                path: vec![id("caddy")]
            }
        );
    }

    /// The other thing only an unvalidated spec can carry, and the one that must **not** be read as
    /// a loop: an edge written twice is one edge. Counted twice it would leave `php-fpm` waiting on
    /// a `mariadb` that can only ever be discharged once, which is indistinguishable from a cycle
    /// from the inside — and would be reported as one, with no loop to name.
    #[test]
    fn a_dependency_listed_twice_is_one_edge_rather_than_a_loop() {
        let graph = ServiceGraph::new([
            unchecked("php-fpm", &["mariadb", "mariadb"]),
            spec("mariadb", &[]),
        ])
        .expect("a repeated edge is still one edge");

        assert_eq!(
            names(
                graph
                    .dependencies_of(&id("php-fpm"))
                    .expect("a known service")
            ),
            vec!["mariadb"],
            "the graph holds edges, not the list as it was written"
        );
        assert_eq!(
            names(graph.start_order().flat()),
            vec!["mariadb", "php-fpm"]
        );
        assert_eq!(names(graph.stop_order().flat()), vec!["php-fpm", "mariadb"]);
    }

    #[test]
    fn a_dependency_that_is_not_here_is_refused_rather_than_ignored() {
        let error = ServiceGraph::new([spec("php-fpm", &["mariadb"])])
            .expect_err("mariadb is not declared");

        assert_eq!(
            error,
            GraphError::UnknownDependency {
                service: id("php-fpm"),
                dependency: id("mariadb"),
            }
        );
    }

    #[test]
    fn the_same_id_twice_is_refused_rather_than_merged() {
        let error = ServiceGraph::new([spec("caddy", &[]), spec("caddy", &[])])
            .expect_err("one of the two would have won silently");

        assert_eq!(error, GraphError::Duplicate { id: id("caddy") });
    }

    /// A graph of nothing is a legal graph — it is what a fresh install has — and every question
    /// must answer rather than panic.
    #[test]
    fn an_empty_graph_answers_everything() {
        let graph = ServiceGraph::new([]).expect("nothing cannot contain a loop");

        assert!(graph.is_empty());
        assert_eq!(graph.len(), 0);
        assert!(graph.start_order().is_empty());
        assert!(graph.stop_order().is_empty());
        assert!(graph.spec(&id("caddy")).is_none());
    }

    /// The same specs in a different order must produce the same plan, or a boot order is a coin
    /// toss and a dependency bug only reproduces on somebody else's machine.
    #[test]
    fn the_plan_does_not_depend_on_the_order_the_specs_arrived_in() {
        let forwards = graph(&[
            ("caddy", &["php-fpm"]),
            ("mailpit", &[]),
            ("mariadb", &[]),
            ("php-fpm", &["mariadb"]),
        ]);
        let backwards = graph(&[
            ("php-fpm", &["mariadb"]),
            ("mariadb", &[]),
            ("mailpit", &[]),
            ("caddy", &["php-fpm"]),
        ]);

        assert_eq!(forwards.start_order(), backwards.start_order());
        assert_eq!(forwards.stop_order(), backwards.stop_order());
    }
}
