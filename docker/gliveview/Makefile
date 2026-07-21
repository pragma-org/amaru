IMAGE := gliveview

.PHONY: dev build

dev: build
	docker run --rm -it \
		--add-host host.docker.internal:host-gateway \
		-v gliveview-blocklog:/opt/cardano/blocklog $(if $(PROM_HOST),--env PROM_HOST=$(PROM_HOST)) $(if $(PROM_PORT),--env PROM_PORT=$(PROM_PORT)) $(if $(PROM_HOST),--env PROM_HOST=$(PROM_HOST)) $(if $(PROM_PORT),--env PROM_PORT=$(PROM_PORT)) \
		$(IMAGE)

build:
	docker build $(if $(AMARU_NETWORK),--build-arg AMARU_NETWORK=$(AMARU_NETWORK)) $(if $(AMARU_NETWORK),--build-arg AMARU_NETWORK=$(AMARU_NETWORK)) -t $(IMAGE) .
