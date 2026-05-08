docker buildx build --build-arg NGX_VERSION=1.29.8 \
    -f Dockerfile-build --target export --output type=local,dest=build .
