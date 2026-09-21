# Encryption key and secret rotation runbook

Refs #210, #220. The gateway writes secrets once and keeps them valid
indefinitely unless they are rotated. This runbook is the operational half of
the remediation; the direct-key deployment mode that makes rotation practical
is documented in `fernet-direct-key-deployment.md`.

## Secret inventory

| Secret | Env var | Consumers | Rotation mechanism |
| --- | --- | --- | --- |
| Gateway data encryption key | `AETHER_GATEWAY_DATA_ENCRYPTION_KEY` (preferred), `ENCRYPTION_KEY` (legacy fallback) | Fernet encryption of provider credentials, OAuth tokens, catalog secrets | Direct-key mode + decryption fallbacks (below) |
| JWT signing key | `JWT_SECRET_KEY` | Session/admin token signing | Rolling restart; sessions invalidate |
| Postgres password | `DB_PASSWORD` (+ `DATABASE_URL`) | Gateway, compose Postgres | Two-phase: DB first, then gateway |
| Redis password | `REDIS_PASSWORD` (+ `REDIS_URL`) | Gateway, compose Redis | Two-phase: Redis first, then gateway |
| Initial admin password | `ADMIN_PASSWORD` (install only) | First-login bootstrap | Rotate via admin UI or env reinstall |
| Backup encryption key | `AETHER_BACKUP_ENCRYPTION_KEY` | `aether-backup-restore` | Keyring overlap via historical keys |
| Tunnel release signing keys | `AETHER_TUNNEL_RELEASE_*` | Tunnel self-upgrade trust | `issue-205-release-key-rotation.md` |

The env file is `/etc/aether/aether-gateway.env` (single-node) or the compose
project env file, mode `0600`/`0640`. Never paste these values into tickets,
chat, CI logs, or shell history; edit the file as root on the host.

## Gateway data encryption key rotation

Prerequisite: the deployment uses direct-key mode. If it still uses
passphrase mode, migrate first (`fernet-direct-key-deployment.md`).

The rotation primitive is decrypt-with-fallback. The primary key encrypts new
ciphertexts; the fallback chain additionally tries every distinct non-empty
value of `AETHER_GATEWAY_DATA_ENCRYPTION_KEY` and `ENCRYPTION_KEY` that
differs from the primary (`fallback_encryption_keys`,
`crates/aether-provider/transport/src/snapshot_mapping.rs:228-244`, and the
catalog equivalent `decrypt_catalog_secret_with_fallbacks`,
`apps/aether-gateway/src/handlers/shared/catalog.rs:58-88`). A rotated-out
key therefore keeps decrypting old rows as long as it stays configured.

### Procedure

1. **Generate the new key** (see the direct-key doc) and record its
   SHA-256 fingerprint in the deployment's release record, e.g.
   `printf '%s' '<new key>' | shasum -a 256`. Store the key itself in the
   secret manager, not the record.
2. **Stage the overlap.** In the env file set
   `AETHER_GATEWAY_DATA_ENCRYPTION_KEY=<new>` and keep the old key as
   `ENCRYPTION_KEY=<old>`. If the old key is itself the gateway-specific
   variable today, write the new value into `AETHER_GATEWAY_DATA_ENCRYPTION_KEY`
   and move the old value to `ENCRYPTION_KEY`.
3. **Restart the gateway.** Startup logs a "differs from ENCRYPTION_KEY"
   warning (`main.rs:2372`); that warning is expected during overlap.
4. **Smoke-test decryption.** Exercise reads that decrypt stored secrets:
   list provider keys in the admin UI, run a quota refresh for an OAuth-based
   provider, and run one proxied request per credential family. Any row that
   fails with an authentication/signature error means the old key is wrong —
   restore the previous env and investigate before proceeding.
5. **Rewrite ciphertexts (optional, for full retirement).** There is no
   batch re-encryption tool; ciphertexts migrate to the new key lazily when
   the owning record is rewritten (credential updates, OAuth refreshes,
   catalog writes). To force retirement of the old key sooner, cycle the
   affected credentials through the admin UI (edit + save each provider key,
   which re-encrypts under the primary key). Decide the acceptable overlap
   window based on how quickly secrets churn in your deployment.
6. **Remove the old key.** Only after every ciphertext is verified rewritten
   (or the overlap window policy expires), delete `ENCRYPTION_KEY=<old>` and
   restart. Confirm no decrypt failures appear in logs for a full business
   day before destroying the old key material from the secret manager.

### Failure modes

- Removing the old key too early: rows still encrypted under it become
  unreadable (`InvalidTokenSignature` at decrypt time). The data is not
  recoverable without the old key — this is fail-closed by design.
- Setting the same value in both variables: harmless; the fallback chain
  deduplicates and startup stops warning.
- Using a value under 32 bytes or a published development example: startup
  validation rejects it (`main.rs:148-160`).

## JWT secret rotation

1. Generate `openssl rand -base64 32` into `JWT_SECRET_KEY`.
2. Rolling-restart the gateway (compose: recreate app containers; systemd:
   `systemctl restart aether-gateway`). All existing sessions and admin tokens
   signed with the old key invalidate; users log in again. There is no token
   overlap: plan the rotation for a low-traffic window or accept forced
   re-authentication.

## Database and Redis password rotation

Compose deployments inject `DB_PASSWORD` into Postgres at container
initialization (`docker-compose.yml:16`) and `REDIS_PASSWORD` via
`--requirepass` (`docker-compose.yml:54`). Rotating only the env file breaks
the running datastore, so always update the datastore first.

1. **Postgres:** pick a maintenance window. As the superuser, run
   `ALTER USER <app user> WITH PASSWORD '<new>';` (and the migration user if
   used), update `DB_PASSWORD`/`DATABASE_URL` in the env file, then recreate
   the gateway app container (do not restart the Postgres container on a
   password-only change). Verify with `psql` from the gateway network.
2. **Redis:** `CONFIG SET requirepass '<new>'` (and update any replica links),
   then update `REDIS_PASSWORD`/`REDIS_URL` and restart the gateway. A Redis
   restart is safe: the default profile is non-persistent
   (`redis-runtime-runbook.md`).
3. Single-node systemd installs only need the env file update + restart,
   because Postgres/Redis are external to the gateway unit.

## Backup encryption key rotation

`aether-backup-restore` reads `AETHER_BACKUP_ENCRYPTION_KEY` and falls back
to the gateway data key. Historical backup keys are supplied as a keyring via
`AETHER_BACKUP_HISTORICAL_KEYS_JSON` so old backup archives stay restorable
while new archives take the new key. Rotate by generating a new key, adding
the old key to the historical JSON keyring, and pruning retired keys only
after their backups have aged out of the retention window. Drill a restore
with the new key before considering the rotation complete
(`backup-restore-drill.md`).

## Admin and tunnel keys

- `ADMIN_PASSWORD` seeds the first admin account at install time. Rotate
  afterwards through the admin UI (preferred) or by reinstalling; the env
  value only matters on first boot.
- Tunnel release signing keys (Ed25519 trust set) follow their own runbook:
  `issue-205-release-key-rotation.md`.

## Cadence and records

- Rotate the gateway data encryption key at least annually and whenever key
  custody is in doubt (personnel change, suspected env file exposure).
- Rotate JWT/DB/Redis credentials at the same annual cadence or per your
  compliance baseline.
- Every rotation records: date, operator, secret fingerprint (never the
  value), affected hosts, verification evidence (smoke-test results), and the
  overlap-window decision.
