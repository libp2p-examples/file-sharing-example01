# Rust-libp2p 的文件传输示例

使用`libp2p`的`rendezvous`, `identify`, `request-response`协议，以及自定义的协议`libp2p-file-get`，构建了一个文件分享的示例。
本示例旨在学习`libp2p`, 目前处于非常不完善的状态。

## 提供的功能

* 使用命令`register <USERNAME>`向`rendezvous`注册，将自身的 `Multiaddress` 信息注册到以 `<USERNAME>` 命名的表空间下
* 使用命令`ls_peers`从`rendezvous`查询所有表空间的信息。注意：表空间就是register命令里使用的参数`<USERNAME>`
* 使用命令`ls@<USERNAME>`获取文件的列表
* 使用命令`get@<USERNAME>-><FILENAME>`获取指定的文件

## 使用

### 运行 `rendezvous` 服务

clone `rust-libp2p` 仓库到本地，并编译运行 `rendezvous`
```bash
git clone https://github.com/libp2p/rust-libp2p.git
git checkout v0.50.0
RUST_LOG=info cargo run -p libp2p-rendezvous --example rendezvous_point
```

### 运行节点1

新打开一个终端, 运行如下命令: 
```bash
RUST_LOG=info cargo run --bin file-sharing -- --sharing-dir=<DIR1> --rendezvous-point-address=/ip4/127.0.0.1/tcp/62649
```
启动以后, 在命令行输入
```bash
register user1
```

### 运行节点2
新打开一个终端, 运行如下命令: 
```bash
RUST_LOG=info cargo run --bin file-sharing -- --sharing-dir=<DIR2> --rendezvous-point-address=/ip4/127.0.0.1/tcp/62649
```
启动以后, 在命令行输入
> 其中`user1`, `user2`是两个不同的用户名, `<DIR1>`, `<DIR2>`是两个不同目录, 如: `/tmp/dir01`, `/tmp/dir02`
```bash
register user2
ls_peers
ls@user1
get@user1->tmp_file.txt
```
等待 `get@user1->tmp_file.txt` 运行完成, 在`<DIR2>`文件夹中查看从`user1`那里获取到的文件.
