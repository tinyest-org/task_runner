

build-builder:
    docker build -t task-builder -f Dockerfile.builder .

build: build-builder
    docker build -t task-runner -f Dockerfile .