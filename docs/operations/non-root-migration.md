# Non-root container migration

Gateway app containers run as UID/GID `10001:10001`. Writable runtime
directories are owned by that identity; production Compose remains read-only
and grants no `DAC_OVERRIDE` or `FOWNER` capabilities.

For an existing named volume, stop the app, migrate ownership, then start it:

```sh
docker compose -f docker-compose.release-local.yml stop release-local-app
./migrate_container_volume_ownership.sh aether_release_local_root
docker compose -f docker-compose.release-local.yml up -d release-local-app
```

The helper validates the volume name and only changes ownership. Reverse it
with `./migrate_container_volume_ownership.sh --rollback VOLUME` after
stopping the app. Back up the volume using the normal backup runbook first.
