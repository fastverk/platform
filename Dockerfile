# service-finder image — multi-stage Cargo build → distroless/cc runtime.
#
# The house style is a Bazel-built OCI image (see BUILD.bazel `rust_service_image`
# + tools/oci), but — exactly like forge's — that path is wired on RBE and is a
# follow-up. This Dockerfile is the immediate build path used to stand the
# service up:
#   docker buildx build --platform linux/amd64 \
#     -t <ECR>/service-finder:<sha> --push .
# distroless/cc carries glibc + ca-certs (the binary links glibc dynamically).
FROM rust:1.95-slim-bookworm AS build
WORKDIR /src
# protoc for build.rs (tonic-build compiles proto/fastverk/finder/v1/finder.proto).
RUN apt-get update \
    && apt-get install -y --no-install-recommends protobuf-compiler pkg-config \
    && rm -rf /var/lib/apt/lists/*
COPY . .
RUN cargo build --release --bin service-finder-server

FROM gcr.io/distroless/cc-debian12
COPY --from=build /src/target/release/service-finder-server /app/service-finder-server
EXPOSE 50060
ENTRYPOINT ["/app/service-finder-server"]
