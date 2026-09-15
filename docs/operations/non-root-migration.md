# Non-root container migration

Gateway app containers run as UID/GID `10001:10001`. Writable runtime
directories are owned by that identity; production Compose remains read-only
and grants no `DAC_OVERRIDE` or `FOWNER` capabilities.

For an existing named volume, stop the app, migrate ownership, then start it:

```sh
docker compose -f docker-compose.release-local.yml stop release-local-app
./migrate_container_volume_ownership.sh aether-release-local_aether_release_local_root
docker compose -f docker-compose.release-local.yml up -d release-local-app
```

The Compose project name makes the actual volume name
`aether-release-local_aether_release_local_root`. If `-p` or
`COMPOSE_PROJECT_NAME` overrides the project name, use the corresponding
prefixed volume from `docker compose ... config --volumes`. The helper
validates the volume name and only changes ownership. Reverse it with
`./migrate_container_volume_ownership.sh --rollback VOLUME` after stopping
the app. The `--rollback` form restores root (`0:0`) ownership for the
legacy runtime layout; it is not a snapshot restore for custom or mixed
ownership. Back up the volume using the normal backup runbook first.
