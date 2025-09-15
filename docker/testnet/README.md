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
db-synthesizer --config p1-config/configs/configs/config.json --bulk-credentials-file bulk.json -s "$(( 86400 * 4 + 1 ))" --db db
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
amaru convert-ledger-state --network testnet_42 --snapshot ledger.snapshot.1.86392.1d38de4ffae6090c24151578d331b1021adb8f37d158011616db4d47d1704968 --snapshot ledger.snapshot.2.172786.932b9688167139cf4792e97ae4771b6dc762ad25752908cce7b24c2917847516  --snapshot ledger.snapshot.3.259174.a07da7616822a1ccb4811e907b1f3a3c5274365908a241f4d5ffab2a69eb8802
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
cat amaru-data/testnet\:42/headers.json | jq -r '.[] | split(".") | join(",")' | while IFS=, read -ra hdr; do  curl http://localhost:9003/blocks/${hdr[0]}/${hdr[1]}/header | xxd -r -p > "amaru-data/testnet_42/headers/header.${hdr[0]}.${hdr[1]}.cbor" ; done
```

import headers:

```bash
../../target/debug/amaru import-headers --config-dir amaru-data --network testnet_42
```


Check the pools' state

```bash
docker exec -it p1 cardano-cli conway query pool-state --testnet-magic 42 --socket-path /state/node.socket --all-stake-pools
```

```
{
    "14f3abc153c13b3d8681e0e0bbde70594ac27f70d156a3727aac184a": {
        "futurePoolParams": null,
        "poolParams": {
            "cost": 340000000,
            "margin": 0,
            "metadata": null,
            "owners": [],
            "pledge": 0,
            "publicKey": "14f3abc153c13b3d8681e0e0bbde70594ac27f70d156a3727aac184a",
            "relays": [
                {
                    "single host name": {
                        "dnsName": "p1.example",
                        "port": 3001
                    }
                },
                {
                    "multi host name": {
                        "dnsName": "p1.example"
                    }
                }
            ],
            "rewardAccount": {
                "credential": {
                    "keyHash": "646b9822097ce55cc8ee518e3588d5da975708029f38918568c82746"
                },
                "network": "Testnet"
            },
            "vrf": "ead46578f501e6a26edd8ea62683d72b736010ff46d4756c24a4eb9a00b55050"
        },
        "retiring": null
    },
    "193d546d81b491f6133683b673f960e3107e38dd658ed1d59e781acb": {
        "futurePoolParams": null,
        "poolParams": {
            "cost": 340000000,
            "margin": 0,
            "metadata": null,
            "owners": [],
            "pledge": 0,
            "publicKey": "193d546d81b491f6133683b673f960e3107e38dd658ed1d59e781acb",
            "relays": [
                {
                    "single host name": {
                        "dnsName": "p2.example",
                        "port": 3001
                    }
                },
                {
                    "multi host name": {
                        "dnsName": "p2.example"
                    }
                }
            ],
            "rewardAccount": {
                "credential": {
                    "keyHash": "3cda1fb28dde415d316bb1fd6a4dfed1c5b093acde7177a5c3d37d94"
                },
                "network": "Testnet"
            },
            "vrf": "4b556dd5dcb0b125fc0583775327d3de98c6d9c08e6fbf15e0966ab2d2e8e4c6"
        },
        "retiring": null
    },
    "2cd5c008f534a9133125f8376066657b7ccd0ec6df74a9246520030a": {
        "futurePoolParams": null,
        "poolParams": {
            "cost": 340000000,
            "margin": 0,
            "metadata": null,
            "owners": [],
            "pledge": 0,
            "publicKey": "2cd5c008f534a9133125f8376066657b7ccd0ec6df74a9246520030a",
            "relays": [
                {
                    "single host name": {
                        "dnsName": "p3.example",
                        "port": 3001
                    }
                },
                {
                    "multi host name": {
                        "dnsName": "p3.example"
                    }
                }
            ],
            "rewardAccount": {
                "credential": {
                    "keyHash": "d488951068eba9e16e0bfef916a02fab6b370a953f4a9f5ad292b753"
                },
                "network": "Testnet"
            },
            "vrf": "034c8f0228c94e86b4875297bd3495dd382b149b0747121611b946649a349901"
        },
        "retiring": null
    },
    "30cbb90cdb8ac8195a640da806dfefbd503e85a089b7bea90bc8adfc": {
        "futurePoolParams": null,
        "poolParams": {
            "cost": 340000000,
            "margin": 0,
            "metadata": null,
            "owners": [],
            "pledge": 0,
            "publicKey": "30cbb90cdb8ac8195a640da806dfefbd503e85a089b7bea90bc8adfc",
            "relays": [
                {
                    "single host name": {
                        "dnsName": "p4.example",
                        "port": 3001
                    }
                },
                {
                    "multi host name": {
                        "dnsName": "p4.example"
                    }
                }
            ],
            "rewardAccount": {
                "credential": {
                    "keyHash": "4375a50afd5a3dc611277e138e0ece5ce65a1cd89c9a2544fe6672a1"
                },
                "network": "Testnet"
            },
            "vrf": "1b5c9fbf842ad08f3e4702d28ba0902e4caa0bd26d12327851d87bc0752838fa"
        },
        "retiring": null
    },
    "878a923b3fa231629261eb917063194385a6228e88d16572c006e31c": {
        "futurePoolParams": null,
        "poolParams": {
            "cost": 340000000,
            "margin": 0,
            "metadata": null,
            "owners": [],
            "pledge": 0,
            "publicKey": "878a923b3fa231629261eb917063194385a6228e88d16572c006e31c",
            "relays": [
                {
                    "single host name": {
                        "dnsName": "p5.example",
                        "port": 3001
                    }
                },
                {
                    "multi host name": {
                        "dnsName": "p5.example"
                    }
                }
            ],
            "rewardAccount": {
                "credential": {
                    "keyHash": "36f97fd53387aaf29ab5db00ff96adb60a441ac4059a7060a3412bef"
                },
                "network": "Testnet"
            },
            "vrf": "121cd697a196729949b2d80f7c59f05c6b4f68e7c001c5d89ff1cdc06cfca2c1"
        },
        "retiring": null
    }
}
```
