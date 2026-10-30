# syntax=docker/dockerfile:1
# CF Best 无窗口版：局域网服务 + 网页管理（cfbest-cli serve），用于 NAS（群晖、极空间、飞牛等）与 Linux 服务器。
#
#   docker build -t cf-best .
#   docker run -d --name cf-best --restart unless-stopped -p 27208:27208 -v ./data:/data cf-best
#
# 构建产物是静态链接（musl）的单个可执行文件；`--target binary --output type=local,dest=out`
# 只导出这个文件，发布流程用它作为 Linux 版下载。

FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY . .
RUN cargo build --release --locked --no-default-features --bin cfbest-cli \
    && strip target/release/cfbest-cli

FROM scratch AS binary
COPY --from=build /src/target/release/cfbest-cli /

FROM alpine:3
# 订阅与测速走 HTTPS 需要根证书；每日定时按 TZ 指定的时区计算。
RUN apk add --no-cache ca-certificates tzdata
COPY --from=build /src/target/release/cfbest-cli /usr/local/bin/cfbest-cli
ENV CFBEST_DATA_DIR=/data \
    TZ=Asia/Shanghai
VOLUME /data
EXPOSE 27208
HEALTHCHECK --interval=1m --timeout=5s --start-period=10s \
    CMD wget -qO /dev/null http://127.0.0.1:27208/healthz || exit 1
ENTRYPOINT ["cfbest-cli"]
CMD ["serve"]
