# Amaru testnet

Spawns a local testnet comprised of:

* 5 block producing cardano-nodes,
* 2 amaru nodes.

## Creation

### Generate testnet configuration

### Generating DB

Collect keys of all configured block forgers:

```bash
( echo "[" ; for i in $(seq 1 5); do
   out="["
   out="${out}$(cat p$i-config/configs/keys/opcert.cert)"
   out="${out},$(cat p$i-config/configs/keys/vrf.skey)"
   out="${out},$(cat p$i-config/configs/keys/kes.skey)]"
   echo $out
   [[ $i -ne 5 ]] && echo ","
done ; echo "]" ) > bulk.json
```

Generate a test database spanning at least 3 epochs:

```bash
db-synthesizer --config p1-config/configs/configs/config.json --bulk-credentials-file bulk.json -s "$(( 86400 * 3 + 1 ))" --db db
```

This should create a `db/` directory containing the chain database and also ledger snapshots:

```
$ ls -alrt ledger.snapshot.*
-rw-rw-r-- 1 curry curry 10485 Sep 11 15:14 ledger.snapshot.1.86392.1d38de4ffae6090c24151578d331b1021adb8f37d158011616db4d47d1704968
-rw-rw-r-- 1 curry curry 12278 Sep 11 15:15 ledger.snapshot.2.172786.932b9688167139cf4792e97ae4771b6dc762ad25752908cce7b24c2917847516
-rw-rw-r-- 1 curry curry 17079 Sep 11 15:16 ledger.snapshot.3.259174.a07da7616822a1ccb4811e907b1f3a3c5274365908a241f4d5ffab2a69eb8802
```

### Generate Amaru snapshots

Convert ledger state

```bash
amaru convert-ledger-state --network testnet:42 --snapshot ledger.snapshot.1.86392.1d38de4ffae6090c24151578d331b1021adb8f37d158011616db4d47d1704968 --snapshot ledger.snapshot.2.172786.932b9688167139cf4792e97ae4771b6dc762ad25752908cce7b24c2917847516  --snapshot ledger.snapshot.3.259174.a07da7616822a1ccb4811e907b1f3a3c5274365908a241f4d5ffab2a69eb8802
```

list blocks in the DB

```
db-analyser --db db --show-slot-block-no --v2-in-mem cardano --config p2-config/configs/configs/config.json
```

Start [db-server](https://github.com/pragma-org/db-server) pointing at generated DB:

```bash
db-server --config p1-config/configs/configs/config.json --db db
```

Start [db-server](https://github.com/pragma-org/db-server) pointing at generated DB:

```bash
db-server --config p1-config/configs/configs/config.json --db db
```

reading `headers.json` file using `db-server`:

```bash
cat amaru-data/testnet\:42/headers.json | jq -r '.[] | split(".") | join(",")' | while IFS=, read -ra hdr; do  curl http://localhost:9003/blocks/${hdr[0]}/${hdr[1]}/header | xxd -r -p > "amaru-data/testnet:42/headers/header.${hdr[0]}.${hdr[1]}.cbor" ; done
```

import headers:

```bash
../../target/debug/amaru import-headers --config-dir amaru-data --network testnet:42
```
