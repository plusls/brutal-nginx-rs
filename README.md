# brutal-nginx-rs

为 nginx stream 和 http 连接设置 brutal TCP 拥塞控制算法。

## 配置指令

本模块提供三条指令，参数都支持写成固定值或 nginx 变量。

- `brutal on|off|$variable;`：启用或关闭 brutal。
- `brutal_rate <rate|$variable>;`：必填，单位为 bytes/s；`104857600` 表示 100 MiB/s。
- `brutal_cwnd_gain <gain|$variable>;`：可选，未配置时默认 `15`。

三条指令都遵循下层配置覆盖上层配置的规则。stream 可以写在 `stream` 或 `server` 中；http 可以写在 `http`、`server` 或 `location` 中，多个 `location` 可以分别配置。

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
    brutal_rate 104857600;
    brutal_cwnd_gain 15;
    proxy_pass $backend;
    proxy_protocol on;
    ssl_preread on;
}
```

### http

```nginx
http {
    brutal on;
    brutal_rate 104857600;
    brutal_cwnd_gain 15;

    upstream app_backend {
        server 127.0.0.1:8080;
    }

    server {
        listen 80;
        server_name example.com;

        location / {
            proxy_pass http://app_backend;
        }

        location /download/ {
            brutal on;
            brutal_rate 209715200;
            brutal_cwnd_gain 20;
            proxy_pass http://app_backend;
        }

        location /static/ {
            brutal off;
            proxy_pass http://app_backend;
        }
    }
}
```

## 运行前置条件

运行环境需要支持 brutal TCP 拥塞控制算法，并且内核或相关模块需要实现自定义 sockopt `TCP_BRUTAL_PARAMS = 23301`。可以先确认系统已暴露 brutal 算法，例如检查 `/proc/sys/net/ipv4/tcp_available_congestion_control` 中是否包含 `brutal`。

本模块只对 TCP 连接设置 brutal；Unix socket 或不支持 brutal 的运行环境会跳过或在日志中记录 `setsockopt` 失败，并让连接继续走现有拥塞控制。

## 详细设计

### 配置继承和变量

三条指令共用同一套配置结构。解析配置时会先尝试把参数解析为静态值；如果参数里包含 nginx 变量，则保存为 complex value，并在请求或连接处理阶段再求值。

配置合并遵循 nginx 常规的下层覆盖上层规则：下层没有显式配置时继承上层配置，下层显式配置后覆盖对应字段。因此 stream 的 `server` 可以覆盖 `stream`，http 的 `server` 可以覆盖 `http`，`location` 可以覆盖 `server`。

### stream 阶段

stream 模块把 handler 插入在 Preread 阶段，但不会在这个阶段解析变量和设置 brutal。原因是 `ssl_preread` 的解析位于 Preread 的最后，相关变量是 cached 的；如果在变量初始化完成前读取，后续可能一直拿到未初始化状态。另一方面，`ssl_preread` 模块执行结束后会直接推进到下一阶段，无法在 Preread 阶段靠更靠后的 handler 完成处理。

因此 stream 模块在 Preread 阶段只做 content handler 包装：保存原 content handler，并替换成自己的 wrapper。真正进入 Content 阶段后，再解析 `brutal`、`brutal_rate`、`brutal_cwnd_gain`，设置 TCP 拥塞控制参数，然后继续调用原 content handler。

### http 阶段

http 模块把 handler 插入在 Access 阶段。HTTP 请求进入 Access 阶段时，server/location 配置已经确定，可以直接解析变量并设置 TCP 拥塞控制参数。

## 构建模块

会自动构建到 build 目录下。

```
docker buildx build --build-arg NGX_VERSION=1.29.8 -f Dockerfile-build --target export --output type=local,dest=build .
```

默认构建产物的 ABI 目前期望兼容对应版本的 Docker 官方 nginx 镜像，例如 `nginx:${NGX_VERSION}-trixie`；实际使用前建议按下方 ABI 校验确认。

如果需要额外 nginx 模块、额外 configure 参数，或需要匹配非官方镜像的 nginx binary，请自行拉取对应 nginx 源码，并使用与目标 nginx binary 一致的 configure 参数配合编译。

## Release 下载

GitHub Release 会提供预构建压缩包，文件名包含构建目标 nginx 版本，例如：

```
brutal-nginx-rs-nginx-1.29.8-v0.1.0.tar.gz
```

压缩包内容：

+ `ngx_brutal_rs_module.so`：release 构建，推荐常规部署使用
+ `debug_ngx_brutal_rs_module.so`：debug 构建，用于排查问题
+ `SHA256SUMS`：上述两个 `.so` 的 SHA-256 校验值

Release 产物只保证匹配构建时的 nginx ABI；当前预构建包面向 `nginx:1.29.8-trixie` 这一类官方镜像 ABI。使用前建议按下方 ABI 校验确认；如果目标 nginx 不是官方镜像，或编译时使用了不同 configure 参数，建议自行构建。

## ABI 校验

可以通过模块中的 ABI 标记确认模块和 nginx binary 是否匹配：

```
strings ./build/ngx_brutal_rs_module.so | grep -E '^[0-9]+,[0-9]+,[0-9]+,[01]{20,}$'
```

同一条命令也可以对 nginx binary 使用：

```
strings /usr/sbin/nginx | grep -E '^[0-9]+,[0-9]+,[0-9]+,[01]{20,}$'
```

两边输出一致时，说明模块 ABI 与目标 nginx binary 匹配。

## 产物

本地构建会在 `build/` 下生成两个模块文件：

+ `ngx_brutal_rs_module.so`：release 构建，推荐常规部署使用
+ `debug_ngx_brutal_rs_module.so`：debug 构建，用于排查问题

两个 `.so` 都包含如下 nginx 模块：

+ ngx_stream_brutal_module
+ ngx_http_brutal_module

既可以作用在 stream 配置中, 又可以作用在 http 配置中

## 项目结构

+ build.rs: https://github.com/nginx/ngx-rust/blob/main/build.rs

## 参考

+ https://github.com/nginx/ngx-rust
+ https://github.com/sduoduo233/brutal-nginx
