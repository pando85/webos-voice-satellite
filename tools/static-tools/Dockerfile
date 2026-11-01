# Build environment for the static tools (musl, Alpine). Built once per target
# platform (linux/arm64, linux/arm/v7); build.sh passes ALPINE_IMAGE from versions.env.
ARG ALPINE_IMAGE=alpine:3.23
FROM ${ALPINE_IMAGE}
RUN apk add --no-cache build-base linux-headers pkgconf file tar xz bzip2 \
        autoconf automake libtool
WORKDIR /work
