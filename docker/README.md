This directory contains development related docker informations.

## TL;DR

```bash
cd docker/cluster
echo NETWORK=preprod >.env
docker compose up --build
```

## Building a development optimized image

`Dockerfile.amaru` helps create a docker image of Amaru. Do not use in production.

The goal of this image is to help with the development of Amaru. It is not optimized
to run an Amaru node. It is optimized to build an Amaru node from source. It takes
advantage of the docker cache to rebuild the image as fast as possible when code changes.

The image is layered that way:

1. install rust tooling
2. setup the rust toolchain
3. fetch amaru's dependencies
4. compile amaru's dependencies
5. build amaru

Once the docker build cache is setup, changing code in amaru should only trigger the
step 5, taking advantage of the cache for all the previous steps.

To build the image, from the project's root directory:

```bash
docker build . -f docker/Dockerfile.amaru -t dev_amaru
```

## Starting a development cluster

The `cluster/docker-compose.yml` file defines a development cluster. It is composed of:

- one cardano node;
- one amaru node following this cardano node;
- a second amaru node following the first.

To start the cluster with the preprod network, setup a `.env` file specifying preprod as
the network and start the cluster:

```bash
cd docker/cluster
echo NETWORK=preprod >.env
docker compose up --build
```

If you change the code and want to restart the amaru node to take this new code into
account:

```bash
docker compose up -d --build amaru
```

## Limitations

At this time, changing the network once you've started the cluster could be problematic.
The amaru (or cardano) node relies on a docker volume and stores its data in a single
directory. So starting the amaru node on preview if you already started it on preprod
before would create some problems and you would have to, either, drop the volume first
or hack with `docker-compose.yml` to change volume.

If this becomes a real problem, we can rely on the docker compose `PROJECT_NAME`
environment variable to ensure each network get its own docker volume, for instance.
