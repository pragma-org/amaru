# Running actions locally

If for some reason you need or want to run the CI pipeline on your local computer, make sure to provide the following prerequisites:

- Linxu system with up-to-date Docker installation (which on Debian means using Docker’s sources)
- [`act`](github.com/nektos/act) installed for your user

When running `act` for the first time, it will ask you for which image size to use: choose the large one (which is >17GB), otherwise tools will be missing.

## Complications due to the Haskell node docker image

The most tricky part of this whole setup is caused by running the Haskell `intersectmbo/cardano-node` docker image from inside the pipeline.
Meanwhile, the pipeline itself is run inside a docker container that comes with `act`.
This means that

- `/var/run/docker.sock` needs to be accessible to the pipeline scripts

  `act` takes care of mapping the `DOCKER_HOST` into the container, but at least on my system I needed to `chmod o+rw /var/run/docker.sock` to get things working.
  Note that this is a really bad idea if you share your system with some other people that shall not have root access.
  YOU HAVE BEEN WARNED!!!

- The Haskell node’s container will receive `${{ runner.temp }}/db-preprod` mounted at `/db`, which is `/tmp/db-preprod` ON THE HOST SYSTEM

  Therefore we need to use `--container-options '-v /tmp:/tmp'` to make the same physical file storage available to both the Haskell node container and the rest of the github action steps.

- The above means that `/tmp/db-preprod` persists between job runs, which will lead to failures when restoring caches inside the pipeline.

  It is therefore necessary to `rm -rf /tmp/db-preprod` AS ROOT between job executions, otherwise the pipeline will fail with a `tar` error.

- The Haskell node will be exposed on port 3001 on all host interfaces, which obviously does NOT include the virtual network inside the `act` container for the running pipeline.

  We therefore must add `--network=host` to the `--container-options` defined above when running the actual `snapshots` job.

## Preparation: obtain chain db

This step takes a long time (several hours) and needs quite a bit of disk space (>10GB) when executed for the first time.
Subsequent runs are much quicker and will further reduce the saved snapshot size due to pruning old blocks.

First, we need to run

```sh
act -j sync_and_cache --matrix network:preprod --container-options '-v /tmp:/tmp'
```

You can watch the blocks being ingested using `docker logs -f cardano-node-ogmios | grep -v enclosingTime`.
Overall progress can be checked using `curl -s localhost:1337/health | jq .networkSynchronization`.

When it is done, remember to clear `/tmp/db-preprod` (perhaps by moving it aside or backing it up, given that losing it costs several hours of waiting time).
Then run the above job again, which will shrink the saved snapshot by half or more.

## Running the tests

Reading the workflow file it may seem that the compilation result of `amaru` is cached from the `build` job and then reused in the `snapshots` job.
Alas, I could not find a way to run only the `x86_64/linux` build job, and running all of them takes a lot of time.
Then, the cache is used but the most expensive dependencies (looking at you, rocksdb!) is still compiled afresh.
So, your mileage may vary.

The main line for running the snapshots test is

```sh
act -j snapshots --container-options '-v /tmp:/tmp --network=host' --env PEER_ADDRESS=$YOUR_IP:3001
```

`YOUR_IP` needs to be set to an IP address of your host system, so that the `act` container can connect to the docker-proxy forwarding port 3001 to the Haskell node.
