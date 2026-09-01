//! `database.create` — roadmap task **T77a**.
//!
//! Thin, like every handler here: it validates two names, makes sure the instance is up, and hands
//! the work to [`crate::services::databases`]. What it adds is the two refusals a caller can act on
//! — a package with no databases at all, and a service this home does not declare — and it tells
//! them apart, because "no such service: redis@main" would send somebody looking for a service that
//! is right there.
//!
//! **One provisioning at a time per instance** — the T77a design, D10. PostgreSQL's conditional
//! creation reads and then writes, so two callers racing for the same database would put one of them
//! into `database "blog" already exists`. The map here folds the second into waiting for the first,
//! on the precedent of the one [`crate::packages`] holds.

use std::collections::HashMap;
use std::sync::Arc;

use mixengine_core::generate::databases::{Ask, validated_identifier};
use mixengine_proto::{DatabaseAccount, DatabaseCreate, Error, ServiceId};
use tokio::sync::Mutex;

use crate::error::ToWire as _;

/// The `database.*` half of the API.
#[derive(Debug)]
pub(crate) struct Databases {
    /// What declares a service, remembers how its databases are made, and can start it.
    services: Arc<crate::services::Registry>,

    /// This machine, for its credential store.
    host: Arc<dyn mixengine_platform::Host>,

    /// One provisioning at a time per instance — see the module note.
    busy: Mutex<HashMap<ServiceId, Arc<Mutex<()>>>>,
}

impl Databases {
    /// The one of these the API holds.
    pub(crate) fn new(
        services: Arc<crate::services::Registry>,
        host: Arc<dyn mixengine_platform::Host>,
    ) -> Arc<Self> {
        Arc::new(Self {
            services,
            host,
            busy: Mutex::new(HashMap::new()),
        })
    }

    /// `database.create` — make a database and the account that reaches it.
    ///
    /// # Errors
    ///
    /// `invalid_argument` for a name that cannot be one, and for a package with no databases;
    /// `not_found` for a service this home does not declare; `conflict` for an account MixEngine
    /// holds no credential for; `precondition_failed` for an instance that will not start.
    pub(crate) async fn create(&self, asked: &DatabaseCreate) -> Result<DatabaseAccount, Error> {
        // Refused before the instance is started, on `blueprint.capture`'s reasoning: a name that
        // was never going to work should not first cost a database server coming up.
        let database = validated_identifier(&asked.database).map_err(|error| error.to_wire())?;
        let user = validated_identifier(asked.user.as_deref().unwrap_or(&database))
            .map_err(|error| error.to_wire())?;

        let provisioning = self.vocabulary(&asked.service).await?;

        // Held across the start and the statements — design D10.
        let gate = {
            let mut busy = self.busy.lock().await;

            Arc::clone(
                busy.entry(asked.service.clone())
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };
        let _turn = gate.lock().await;

        self.services.ensure_running(&asked.service).await?;

        let ask = Ask { database, user };
        let made =
            crate::services::databases::ensure(&self.host, &provisioning, &asked.service, &ask)
                .await?;

        Ok(DatabaseAccount {
            service: asked.service.clone(),
            database: ask.database,
            secret: provisioning.secret_address(&ask.user),
            user: ask.user,
            made,
        })
    }

    /// How this service's databases are made, or which of the two misses it was.
    async fn vocabulary(
        &self,
        service: &ServiceId,
    ) -> Result<mixengine_core::generate::Provisioning, Error> {
        // The graph first, because it is what fills the registry's map: a daemon that has served
        // nothing yet remembers no provisioning for anything.
        let graph = self
            .services
            .graph()
            .await
            .map_err(|error| error.to_wire())?;

        if graph.spec(service).is_none() {
            return Err(mixengine_core::Error::Graph(
                mixengine_core::services::GraphError::NoSuchService {
                    id: service.clone(),
                },
            )
            .to_wire());
        }

        self.services.provisioning_for(service).ok_or_else(|| {
            mixengine_core::Error::NoDatabaseVocabulary {
                package: service.name().to_owned(),
            }
            .to_wire()
        })
    }
}
