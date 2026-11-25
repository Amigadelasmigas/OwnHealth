#!/bin/sh
# Prove a backup restores: take a restic snapshot (default: the latest), load it into a scratch
# database next to the live one with the profile keys from keyring.json, and compare row counts, blob
# hashes and every profile's state_hash with the manifest written at backup time. Prints the report;
# exits 1 on any difference.
#
#   ./deploy/restic/restore-test.sh            # the latest snapshot
#   ./deploy/restic/restore-test.sh 4be1a2c3   # one snapshot
#   cron: 45 4 * * 0 cd /opt/ownhealth && ./deploy/restic/restore-test.sh >> backup.log 2>&1
#
# The live database and blob store are only read. A backup that was never restored counts as missing.
set -eu
. "$(dirname "$0")/common.sh"
RUN=${OWNHEALTH_RUN-docker compose run --rm -T admin}

# Each run restores into a folder of its own, so a run never deletes the archive that another run's
# check is still reading. A folder whose run started a day ago or more is one a stopped run left.
# Anything else there is a leftover too: versions before 1.0 restored straight into the folder.
HERE="$SCRATCH/run-$(date +%s)-$$"
# shellcheck disable=SC2016 # expanded by the inner shell
$RUN sh -c 'now=$(date +%s); for d in "$1"/* "$1"/.[!.]* "$1"/..?*; do
  [ -e "$d" ] || [ -L "$d" ] || continue
  case ${d##*/} in
    run-*)
      t=${d##*/run-}; t=${t%%-*}
      case $t in "" | *[!0-9]*) ;; *) [ $((now - t)) -ge 86400 ] || continue ;; esac
      ;;
  esac
  rm -rf "$d"
done' sweep "$SCRATCH"
log "restic restore $SNAPSHOT"
$RUN restic restore --quiet --host ownhealth --tag ownhealth "$SNAPSHOT:$STAGING" --target "$HERE" >&2
status=0
$RUN ownhealth verify-restore "$HERE" || status=$?
$RUN rm -rf "$HERE"
if [ "$status" -eq 0 ]; then log "restore test passed"; else log "restore test FAILED"; fi
exit "$status"
