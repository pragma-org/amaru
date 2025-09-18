#!/usr/bin/env bash
set -vxo pipefail

export PATH=/usr/local/bin:$PATH

# Implement sponge-like command without the need for binary nor TMPDIR environment variable
write_file() {
    # Create temporary file
    local tmp_file="${1}_$(tr </dev/urandom -dc A-Za-z0-9 | head -c16)"

    # Redirect the output to the temporary file
    cat >"${tmp_file}"

    # Replace the original file
    mv --force "${tmp_file}" "${1}"
}

copy_config () {
    POOL_ID=$1

    # copy configuration
    cp -fr /data/p${POOL_ID}-config/configs/* /configs/${POOL_ID}/
    chmod 0600 /configs/${POOL_ID}/keys/*
}

copy_database () {
    POOL_ID=$1

    PROTOCOL_MAGIC=$(jq .protocolConsts.protocolMagic /configs/${POOL_ID}/configs/byron-genesis.json)

    # copy database
    cp -fr /data/generated/db/* /state/${POOL_ID}/
    echo -n $PROTOCOL_MAGIC > /state/${POOL_ID}/protocolMagicId

}

config_topology_json() {
    # Generate a ring topology, where pool_n is connected to pool_{n-1} and pool_{n+1}

    # Count number of pools
    VALENCY=2

    local num_pools=$2
    local i prev next

    for ((i=1; i<=num_pools; i++)); do
        prev=$((i - 1))
        if [ $prev -eq 0 ]; then
            prev=$num_pools
        fi

        next=$((i + 1))
        if [ $next -gt $num_pools ]; then
            next=1
        fi

        cat <<EOF > "/configs/$i/configs/topology.json"
{
  "localRoots": [
    {
      "accessPoints": [
        {"address": "p${prev}.example", "port": 3001},
        {"address": "p${next}.example", "port": 3001}
      ],
    "advertise": true,
    "trustable": true,
    "valency": ${VALENCY}
    }
  ],
    "publicRoots": [],
    "useLedgerAfterSlot": 0
}
EOF
    done
}

set_start_time() {
    # set start time 5 * epochs in the past
    SYSTEM_START_UNIX=$(echo "$(date +%s) - (86400 * 4)" | bc)

    SHELLEY_GENESIS_JSON="$1/configs/shelley-genesis.json"
    BYRON_GENESIS_JSON="$1/configs/byron-genesis.json"

    # Convert unix epoch to ISO time
    SYSTEM_START_ISO="$(date -d @${SYSTEM_START_UNIX} '+%Y-%m-%dT%H:%M:00Z')"

    # .systemStart
    jq ".systemStart = \"${SYSTEM_START_ISO}\"" "${SHELLEY_GENESIS_JSON}" | write_file "${SHELLEY_GENESIS_JSON}"

    # .startTime
    jq ".startTime = ${SYSTEM_START_UNIX}" "${BYRON_GENESIS_JSON}" | write_file "${BYRON_GENESIS_JSON}"
}

pools=$(ls -d /configs/*)
number_of_pools=$(ls -d /configs/* | wc -l)
echo "number_of_pools: $number_of_pools"
for pool in $pools; do
  pool_ix=$(echo "$pool" | awk -F '/' '{print $3}')
  echo "configure pool: $pool ($pool_ix)"
  copy_config "$pool_ix"
  config_topology_json "$pool_ix" "$number_of_pools"
  set_start_time "$pool"
done

if [[ -d /data/generated/db ]] ; then
    echo "Generated DB exists, archiving it"
    mv /data/generated/db /data/generated/db.old
fi

# generate DB
# assumes /data/generated exists, should be a volume injected
pushd /data/generated

# collect keys
( echo "[" ; for i in $(seq 1 "$number_of_pools"); do
                 out="["
                 out="${out}$(cat /configs/$i/keys/opcert.cert)"
                 out="${out},$(cat /configs/$i/keys/vrf.skey)"
                 out="${out},$(cat /configs/$i/keys/kes.skey)]"
                 echo $out
                 [[ $i -ne 5 ]] && echo ","
             done ; echo "]" ) > bulk.json

db-synthesizer --config /configs/1/configs/config.json --bulk-credentials-file bulk.json -s "$(( 86400 * 4 ))" --db db
popd

# copy DB
for pool in $pools; do
  pool_ix=$(echo "$pool" | awk -F '/' '{print $3}')
  echo "copy db for pool: $pool ($pool_ix)"
  copy_database "$pool_ix"
done
