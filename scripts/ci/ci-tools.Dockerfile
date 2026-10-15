# CI tools image for the `quality gates` job (.github/workflows/rust.yml).
#
# The Go build cache as an image: a golang stage compiles the five CI
# tools from scripts/ci/go to /tools, and the final scratch stage
# carries only those binaries. The workflow pulls this image
# content-addressed by its inputs (see the job's tools key) and copies
# the binaries to the host with `docker create` + `docker cp`; the
# image is a file carrier and is never run. No Go change = pull + copy
# only, no compile, and CI never needs a Go toolchain on the runners.
ARG GO_VERSION=1.27
FROM golang:${GO_VERSION}-trixie AS build

WORKDIR /src
COPY scripts/ci/go/ ./go/
RUN mkdir -p /tools && cd go \
    && CGO_ENABLED=0 go build -o /tools/coverage-floor ./cmd/coverage-floor \
    && CGO_ENABLED=0 go build -o /tools/coverage-summary ./cmd/coverage-summary \
    && CGO_ENABLED=0 go build -o /tools/geiger ./cmd/geiger \
    && CGO_ENABLED=0 go build -o /tools/complexity ./cmd/complexity \
    && CGO_ENABLED=0 go build -o /tools/rust-quality-gates ./cmd/rust-quality-gates \
    && ls /tools

FROM scratch
COPY --from=build /tools/ /tools/
