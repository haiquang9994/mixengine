# T77a — creating a database and the account that reaches it (design)

Roadmap task **T77a**, phase 8. `PlanAction::CreateDatabase` names something nothing in this
workspace can do: no code anywhere creates a database, an account, or a credential for one that is
not `root`. T78 executes a plan and cannot execute that step until this exists, so it comes first
and on its own.

## Goal

On a running MariaDB, MySQL or PostgreSQL instance, make sure a named database exists and that a
named account can reach it, with the account's password generated here and stored in the OS keyring
— repeatably, so that asking twice changes nothing the second time.

The second half is the one with teeth. "Ensure" is easy to write and easy to write dangerously: the
same call that creates an account can quietly reset the password of an account somebody else made.
This design's load-bearing sentence is D3, and everything else follows from it.

## Scope

In: a `Recipe` hook that declares how a package administers its databases, its three implementations,
a daemon runner that performs one, the `database.create` method, and `mix database create`.

Out: `database.list` and `database.drop` — nothing needs either, and dropping a database is a
different decision with different stakes (D9). Reading a password back out: that is T83's shape, and
building it here would be guessing at it (D11). Writing the credential into a project's `.env`: a
scaffold's job, not MixEngine's. Anything about *browsing* data — that is MixDB, and
[`features/services.md`](../../../.claude/features/services.md) already says so.

## Decisions

**D1 — The recipe declares, the daemon performs. T33's division, one table across.** A recipe lives
in `mixengine-core`, which has no business reaching an OS credential store or spawning a process; the
daemon holds the `Keyring` and the process runner. So a recipe answers *what statements*, and the
daemon answers *with which credential, in what order, against which running server*. This is exactly
the split [`generate::first_run`](../../../crates/mixengine-core/src/generate/first_run.rs) and
[`services/first_run.rs`](../../../crates/mixengine-daemon/src/services/first_run.rs) already draw,
and reusing it means the keyring appears in one place in this workspace rather than two.

Two alternatives were weighed. A separate `DatabaseAdmin` trait beside `Recipe` is tidier on paper
but makes the catalogue keep two tables and makes "what is MariaDB" answerable in two files. A
`match` on the package name inside the daemon is the shortest thing that works today and puts
knowledge of MariaDB's syntax outside MariaDB's recipe, which is the arrangement `CLAUDE.md` exists
to prevent.

**D2 — `Step` moves out of `first_run` into `generate::step`, and `first_run` re-exports it.** Two
kinds of work now describe themselves as a list of programs to run with a deadline and an optional
secret file. Leaving the shape inside the module named *what happens once before a service is ever
started* would make every provisioning step import from a module whose name says it is not one. The
daemon gets the matching move: the `run` function inside `services/first_run.rs` — which writes a
secret file, runs the program, removes the file whatever happened, and turns a non-zero exit into an
error carrying what the program said — becomes `services/step.rs`. Neither move changes behaviour and
both are covered by the tests that already exist.

**D3 — A keyring entry is the deed of ownership, and that is what makes `ALTER USER` safe.** The
account's password lives at `<service-id>/<user>` — the address `Context::secret_address` already
builds, and the same shape `mariadb@main/root` has had since T33. Three cases, and the middle one is
the whole point:

- **The account does not exist.** Generate a password, store it, create the account with it.
- **The account exists and the keyring holds a password for it.** It is ours. Do not rotate:
  keep the stored value and `ALTER USER … IDENTIFIED BY` it, so a server that has drifted is brought
  back into agreement with the credential store. This is self-healing, not password rotation.
- **The account exists and the keyring holds nothing for it.** It is not ours. **Refuse**, naming the
  account, and say that `--user` picks another name. MixEngine does not take over an account it did
  not make.

Without the third branch there is no third branch: `CREATE USER IF NOT EXISTS` followed by
`ALTER USER` silently seizes whatever it lands on. Phase 3 built a two-marker scheme in a data
directory for the smaller version of this question — *is this ours to clear* — and the answer here
costs one read-only query.

**D4 — The probe's output is a canonical word per object; only the query differs per engine.** The
daemon has to know which of the two objects exist before it can decide D3, and the obvious shape —
each recipe emits its own output and the daemon parses it — puts MariaDB's output format in the
daemon and re-opens D1. So each engine's query is written to print the word `database` on a line if
the database exists and `user` on a line if the account does. The reading is one shared function; the
asking is three statements in three recipes.

```sql
-- mariadb, mysql
SELECT 'database' FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = 'blog';
SELECT 'user'     FROM mysql.user WHERE User = 'blog' LIMIT 1;
-- postgres
SELECT 'database' FROM pg_database WHERE datname = 'blog';
SELECT 'user'     FROM pg_roles    WHERE rolname = 'blog';
```

**D5 — The keyring is written before the server is touched, and a half-finished attempt heals.**
T33's ordering, for T33's reason: a machine with no credential store must fail while nothing has been
created. The failure mode this leaves is a keyring entry for an account that does not exist, and it
is the harmless one — the next attempt probes, finds no account, and creates it with exactly the
stored password. The opposite order leaves an account whose password exists nowhere, which nothing
can repair.

**D6 — On PostgreSQL the account owns the database, so its steps run in the other order.** From
PostgreSQL 15, `GRANT ALL ON DATABASE` no longer carries `CREATE` on the `public` schema, so a role
granted everything a MySQL-shaped design would grant it still cannot create a table: the Django
blueprint of T79 would apply cleanly and die on its first migration. The fix is not a schema grant
but ownership — `CREATE DATABASE "blog" OWNER "blog"` — which means the role has to exist first.
MySQL and MariaDB have no such ordering. That the two orders differ is why the step list belongs to
the recipe rather than to a shared sequence in the daemon.

`CREATE DATABASE` has no `IF NOT EXISTS` in PostgreSQL and cannot run inside a transaction, so the
conditional is psql's `\gexec`: `SELECT 'CREATE DATABASE …' WHERE NOT EXISTS (SELECT FROM
pg_database WHERE datname = …)\gexec`.

**A database that already exists does not change owner.** D3's deed covers the account, not the
database, and `ALTER DATABASE … OWNER TO` would hand somebody else's database to our account. The
existing database is granted to the account and reported as `existing`; what that means for an app on
PostgreSQL 15 is the caller's to see, and the response says enough to see it.

**D7 — No statement names a character set or a collation.** The recipes already configure
`character_set` and `collation` on the instance, and a `CREATE DATABASE … CHARACTER SET utf8mb4` here
would be a second place deciding the same thing — one that silently wins, and that nobody would think
to change when the setting moved.

**D8 — `database.create` answers inline rather than returning a job.** Making a database takes
milliseconds; what can take minutes is getting there — the instance may be stopped, and starting it
may run a first-run ritual. `service.start` already carries exactly that cost synchronously, there is
no request timeout in the daemon beyond `HEADER_TIMEOUT` on reading headers, and `mix`'s client sets
none at all. A job here would be a second progress vocabulary for a call whose own work is instant.

**D9 — `create` only, and T78's rollback never drops a database.** This is stated here rather than in
T78 because it is what keeps `database.drop` out of this task. Dropping a database destroys data — by
the time an apply fails, a scaffold may have migrated into it — and phase 3 built a whole marker
scheme so that MixEngine would never remove a database directory it had not made. A rollback that
undoes "what this apply created" therefore stops at the database: it leaves it, and **says** it left
it. Nothing is lost for resumption, because creating is idempotent.

**D10 — One provisioning at a time per instance.** `\gexec` reads and then writes, so two applies
racing for the same database put one of them into `ERROR: database "blog" already exists`. A mutex
keyed by `ServiceId`, on the precedent of the map `packages.rs` holds to fold a second install into
the first.

**D11 — The wire carries the address of the password, never the password.** `mix database create`
prints where the credential is, not what it is: a password on a terminal is a password in scrollback,
in a tmux buffer and in a CI log. Handing one to a program that needs it is T83's design — "read from
the keyring at that moment and never placed in an argument or a URL" — and inventing a second shape
for it here would be a shape T83 has to contradict. The address is derivable (`<service-id>/<user>`),
so the response is telling the caller a rule rather than keeping a secret from it.

**D13 — The last step logs in as the account just made, so the method cannot report a success the
account cannot use.** Every other step here is issued as root; none of them proves that the thing
they built can be reached. So the recipe's list ends with a step that connects **as the new account
to the new database** and creates and drops a table in it — a real table in `public` on PostgreSQL,
which is also the only thing that proves D6's ownership, and a temporary one would not.

The reason this is a step rather than an assertion in a test is
[`tests/mariadb.rs`](../../../crates/mixengine-cli/tests/mariadb.rs)'s own finding: on macOS a
keychain item carries an ACL naming the application that created it, so a **test process** reading
the daemon's credential raises a dialog nobody can answer — measured once at twenty-seven minutes
before the job timed out. The daemon already holds the password, so the proof costs it one local
connection; moving it into the method makes it a postcondition every caller gets rather than an
assertion one suite makes, and leaves the integration tests with nothing to read. It is the same
rule `service.create` follows in refusing to report success for a configuration the server rejected.

**D12 — The plan of T77 goes on saying `Create` for this step, and must not be "fixed" to say
`Satisfied`.** Knowing whether a database exists means asking a running server; T77's D9 says a plan
reads this home's tables and touches nothing else, precisely because it is the command a person runs
when they want nothing to happen. Idempotence lives in the executor. A future change that made the
plan connect would buy an accurate word in a listing and cost the property that `--dry-run` is free.

## Data model

**No migration, and no table.** The SQL server is the record of which databases exist, the keyring is
the record of which accounts are ours, and the address that joins them is derived by rule. A table
here would be a fourth copy that can disagree with the other three, and the one thing that genuinely
needs remembering across a restart — *did this apply create it* — is T78's ledger and is answered by
this method's response.

## API

```rust
// mixengine-proto
pub struct DatabaseCreate {
    pub service: ServiceId,
    pub database: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,      // defaults to the database name
}

pub struct DatabaseAccount {
    pub service: ServiceId,
    pub database: String,
    pub user: String,
    pub secret: String,            // the keyring address, never the password
    pub made: Provisioned,
}

/// What this call did to one object, which is what T78's ledger records.
#[serde(rename_all = "snake_case")]
pub enum Made { Created, Existing }

pub struct Provisioned { pub database: Made, pub user: Made }
```

`database.create`. Errors: `not_found` for a service nothing is registered as; `invalid_argument`
for a name outside the slug charset **and** for a package with no database vocabulary — Redis and
Memcached answer this, and deliberately not `unsupported`, which means *this operating system cannot*
and would be a lie about the machine, exactly as T77's D12 argued for `blueprint.apply`; `conflict`
for D3's third branch; `precondition_failed` for an instance that will not start; whatever a failed
step reports, carrying what the client printed.

**Identifiers are validated, never escaped.** Database and account names are checked against the same
slug charset a project name is, in `mixengine-core`, before any statement is built — so no statement
contains anything that would need quoting, and there is no escaping function to get wrong. The
generated password's alphabet is `mixengine_platform::generate_secret`'s, chosen for exactly this
interpolation when T33 met it first.

**No password reaches a command line.** Statements go in on standard input, as the bootstrap step of
T33 does; the root credential reaches the client through `MYSQL_PWD` / `PGPASSWORD`, which is how the
health checks already reach it.

## CLI

```
mix database create <service> --name <database> [--user <account>] [--json]
```

`db` as a visible alias. The rendering says what happened and where the credential is:

```
database blog created on mariadb@main
account  blog created, password in the keyring at mariadb@main/blog
```

`mix database` is a namespace with one verb and no table behind it. That asymmetry is real and is
recorded rather than papered over with a `list` nothing has asked for.

## Elevation

None. A database server runs as the user, and every statement here goes to a socket that user already
owns.

## Testing

**The statements, without a server.** Each recipe's probe and steps against a fixture context:
golden text per engine; PostgreSQL's role before its database; no `CHARACTER SET` anywhere; and one
test that walks every generated step of every engine and refuses to find the password in `args` —
the shape T77's "what is forbidden does not get out" group takes.

**D3, without a server.** The decision is a pure function of what the probe found and what the
keyring holds, so the three branches are a table test with no process and no database: created,
adopted-and-realigned, refused. The rule the whole task rests on is provable in milliseconds.

**Against real servers, and reading no credential.** Extending
[`tests/mariadb.rs`](../../../crates/mixengine-cli/tests/mariadb.rs),
[`tests/mysql.rs`](../../../crates/mixengine-cli/tests/mysql.rs) and
[`tests/postgres.rs`](../../../crates/mixengine-cli/tests/postgres.rs), which already install and
start a server: `mix database create` succeeds — which by D13 *is* the assertion that the credential
works and that the account can create a table, made by the daemon that owns the credential — then
succeeds again reporting `existing`/`existing`, and a connection as the new account with a wrong
password is refused, which is what these suites already do to prove there is no way in without one.
Nothing in the test process ever asks the keyring for anything. MySQL gets the same three because
"MariaDB syntax and MySQL syntax are one syntax" is an assumption this task makes and should
therefore be one this task checks.

## Dependencies

T33 for the ritual shape, the keyring ordering and the step runner; T31/T32 and T34 for the three
recipes and their clients (`bin/mariadb`, `bin/mysql`, `bin/psql`, all already in `provides`); T70a
because an instance may be idle-stopped when the call arrives, so the runner starts it first.
Nothing here needs the supervisor to change.

## Risks

**The real-server tests are the slow ones.** They download and bootstrap a database; this adds
seconds to suites already measured in minutes, on the CI leg that is already the slowest. Accepted:
the alternative is a feature whose credential path is proved by nothing.

**No credential store, no provisioning.** A headless Linux without a secret service fails here as it
already fails on any first run, through the same message T15b wrote. Nothing new is owed.

**`mysql.user` is a view over `mysql.global_priv` on modern MariaDB.** It is readable as root on both
and on every version this product ships, but it is a compatibility surface rather than a documented
contract, and if it moves the probe is where it is felt.

## Text that this task makes wrong

- [`roadmap/phase-8-differentiators.md`](../../../.claude/roadmap/phase-8-differentiators.md) —
  **T77a** is inserted between T77 and T78, in the order it has to be built.
- [`features/services.md`](../../../.claude/features/services.md) — "Database management scope" lists
  credentials as in scope without saying that creating a database and an account for a project is
  what that means. One sentence, pointing here.
- [`features/client-surface.md`](../../../.claude/features/client-surface.md) — a graphical client
  needs `database.create` reachable; it is added to the surface list, per `CLAUDE.md`'s rule that a
  gap in `mix` is a gap in the product.
- [`features/blueprints.md`](../../../.claude/features/blueprints.md) — the Apply section's "create DB
  `blog`" is now a real operation with a named owner and a keyring address, and the rollback sentence
  gains D9's exception.
