# brutal-nginx-rs

基于 stream 模块的 brutal 拥塞控制算法设置

可以给 stream 设置 brutal

## 样例配置

### stream

只为 www.speedtest.cn 启用该算法

```nginx
map $ssl_preread_server_name $brutal_enable {
    www.speedtest.cn on;
    default       off;
}

map $ssl_preread_server_name $backend {
    www.speedtest.cn 127.0.0.1:10000;
    default       127.0.0.1:10001;
}

server {
    listen 0.0.0.0:443;
    listen [::]:443 ipv6only=on;
    brutal $brutal_enable;
    brutal_rate 104857600; # 100M 必填
    brutal_cwnd_gain 15; # 可选
    proxy_pass $backend;
    proxy_protocol on;
    ssl_preread on;
}
```

### http

同上, 可以写在 http 配置块中, 最低可以写到 location 配置中

## 运行前置条件

运行环境需要支持 brutal TCP 拥塞控制算法，并且内核或相关模块需要实现自定义 sockopt `TCP_BRUTAL_PARAMS = 23301`。可以先确认系统已暴露 brutal 算法，例如检查 `/proc/sys/net/ipv4/tcp_available_congestion_control` 中是否包含 `brutal`。

`brutal_rate` 是必填项，单位与 brutal 内核实现保持一致；当前示例 `104857600` 表示 100M。`brutal_cwnd_gain` 可选，未配置时默认使用 `15`。

本模块只对 TCP stream 连接设置 brutal；Unix socket 或不支持 brutal 的运行环境会跳过或在日志中记录 `setsockopt` 失败，并让连接继续走现有拥塞控制。

## 构建模块

会自动构建到 build 目录下

```
docker buildx build --build-arg NGX_VERSION=1.29.8 -f Dockerfile-build --target export --output type=local,dest=build .
```

## 产物

ngx_brutal_rs_module.so

其中包含如下 nginx 模块

+ ngx_stream_brutal_module
+ ngx_http_brutal_module

既可以作用在 stream 配置中, 又可以作用在 http 配置中

## 开发

```
docker build -f Dockerfile-dev -t ngx-rs-dev .
docker run --rm -it --user 1000 -v .:/work  docker.io/library/ngx-rs-dev bash
```

## 项目结构

+ build.rs: https://github.com/nginx/ngx-rust/blob/main/build.rs

## 参考

+ https://github.com/nginx/ngx-rust
+ https://github.com/sduoduo233/brutal-nginx
