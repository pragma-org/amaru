---
type: architecture
status: accepted
participants: abailly, etorreborre
---

# Version & Migrate Chain Database

## Context

* While Amaru is not yet used in production, there's already been a couple occurrences where developers found themselves stuck with an unusable database following the upgrade of Amaru binary, requiring scrapping the existing data and bootstrapping afresh. While bootstrapping is relatively fast, resynchronising is a lengthy process which one would rather like to avoid
* This behaviour will be unacceptable in production especially as Amaru strives to provide fast turnaround and low footprint, and we need to preserve existing users' data as much as possible
* _Migration_ is a common feature afforded by classical database applications whereby the system checks the consistency of its database at boot time and either offers to migrate the data in case of incompatibility or even migrate it automatically
* Migration tools are rife in the RDBMS world (see [this page](https://wiki.postgresql.org/wiki/Change_management_tools_and_techniques) for a Postgres-centric list) and migration process is typically handled by storing versions and migration scripts results and metadata in the database itself to make it self-checkable, with actual migration code integrated in the executable

## Decision

* We version the _Chain Database_ using a `u16` number, starting with 1
* We write this version in the database under key `__VERSION__`
* The current version is a constant `CHAIN_DB_VERSION` in the code
* When Amaru starts from an existing database it will check its version and if the binary and stored versions don't match, will refuse to start with a message
* We provide a `migrate-chain-db` command that gives user the possibility to migrate an existing database up to the currently supported version
* When bootstrapping Amaru from scratch, we make sure to migrate the (empty) database being bootstrapped
* When migrating a database, we record the execution of each _migration step_ in the database with some metadata (timestamp, git commit, name of step)

## Consequences

* The `CHAIN_DB_VERSION` _must_ be incremented by one every time the database changes, eg. we add some new data, modify how data is stored...
* Developers _must_ provide a _migration script_ (eg. some code) for each increment. The `migrate-chain-db` command will apply each migration script in order to update the database to the latest version
  * Migration scripts should expect the database to be possibly empty
* It could be the case a migration is impossible, in which case this should be reported by the _migration script_ with migration utility reporting back to user and instructing to start from scratch

## Follow-up questions

This EDR does not answer some questions about migrations and database changes that could nevertheless be interesting and impactful for users:

* What about changes in the RocksDB storage format? Or even a migration to a new DB? Is this ever going to happen?
* We should start consolidating our documentation into an "Amaru Run Book" as a comprehensive guide for operators and users
