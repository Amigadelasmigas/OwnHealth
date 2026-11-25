#!/bin/sh
# Restore a restic snapshot (default: the latest) into a fresh OwnHealth stack: a new database volume
# and a new data volume, with the app not started yet. The profile keys come from keyring.json in the
# keyring folder, which is not in the snapshot; without it, or when it lacks a key of the snapshot,
# nothing is loaded. OWNHEALTH_ALLOW_MISSING_KEYS=1 loads every record anyway, and the files only of
# the profiles whose key the keyring holds (deploy/README.md "Restore"). Afterwards it checks every
# profile's state_hash against the manifest and exits 1 on any difference. `docker compose up -d`
# afterwards runs the admin service, which grants the app role its rows on the restored tables. Steps:
# deploy/README.md "Restore".
#
#   docker compose up -d db
#   ./deploy/restic/restore.sh
#   docker compose up -d
set -eu
. "$(dirname "$0")/common.sh"
RUN=${OWNHEALTH_RUN-docker compose run --rm -T admin}
FLAGS=
if [ "${OWNHEALTH_ALLOW_MISSING_KEYS:-}" = 1 ]; then FLAGS=--allow-missing-keys; fi

# The dump carries the app role's grants, so the role must exist before pg_restore loads them.
# compose.yaml mounts app-role.sh into the admin container; elsewhere (OWNHEALTH_RUN="") it is skipped.
ROLE=${OWNHEALTH_APP_ROLE_SCRIPT:-/opt/ownhealth-db/app-role.sh}
# Name the snapshot once: a retry of a restore that stopped must load the same one, and "latest" may
# name a newer snapshot by then (deploy/README.md "Restore").
if ! LIST=$($RUN restic snapshots --json --host ownhealth --tag ownhealth "$SNAPSHOT"); then
  log "restic could not list snapshot $SNAPSHOT (its error is above)"
  exit 1
fi
# restic ignores --host and --tag for an explicit id, so check them here. The list is one line of JSON.
ID=$(printf '%s\n' "$LIST" | tr ',' '\n' | sed -n 's/.*"short_id": *"\([0-9a-f]*\)".*/\1/p')
HOST=$(printf '%s\n' "$LIST" | tr ',' '\n' | sed -n 's/.*"hostname": *"\([^"]*\)".*/\1/p')
TAGS=$(printf '%s\n' "$LIST" | sed -n 's/.*"tags": *\[\([^]]*\)\].*/\1/p')
case $ID in "" | *[!0-9a-f]*) HOST= ;; esac
case "$HOST,$TAGS," in
  'ownhealth,'*'"ownhealth",'*) ;;
  *)
    log "$SNAPSHOT is not one snapshot with host ownhealth and tag ownhealth"
    exit 1
    ;;
esac
# A retry from a new shell must reach the same Compose project.
AGAIN=${COMPOSE_PROJECT_NAME:+COMPOSE_PROJECT_NAME=$COMPOSE_PROJECT_NAME }
START=$AGAIN
if [ -n "$FLAGS" ]; then AGAIN="${AGAIN}OWNHEALTH_ALLOW_MISSING_KEYS=1 "; fi
log "restoring snapshot $ID; to retry this restore, run it again with the same id: ${AGAIN}./deploy/restic/restore.sh $ID"
# Its own run folder, like restore-test.sh, so a restore test that runs meanwhile leaves it alone.
HERE="$SCRATCH/run-$(date +%s)-$$"
$RUN sh -c "rm -rf '$SCRATCH' \
  && { [ ! -f '$ROLE' ] || sh '$ROLE'; } \
  && restic restore --quiet --host ownhealth --tag ownhealth '$ID:$STAGING' --target '$HERE' >&2 \
  && ownhealth restore $FLAGS '$HERE'; status=\$?; rm -rf '$HERE'; exit \$status"
log "restored; start the app with: ${START}docker compose up -d"
