# Farol

[English](README.md)

放在桌上的一盏灯，替你看着 [Paseo](https://paseo.sh) 里的 agent，跑在 ESP-Mosaico 开发板上。
看一眼灯就知道它们怎么样了：都在干活时是冷色、慢慢呼吸；有 agent 在等你时变琥珀色；
一轮失败时变红；光的高低跟着你最紧的那份额度走。灯后面是一叠卡片，一个 agent 一张，
回答问题、权限或计划，停止正在跑的一轮，把完成的标成已读，都不用打开 App。

Farol 由两部分组成，都在这个仓库里：

- 板子上的应用（ESP32-S31，界面由 GSP 渲染，逻辑用 Rust 写）；
- Paseo 插件（`plugin/`）：把 Paseo 的状态整理成给板子的简报，替板子执行动作，执行前先做检查。

## 板子怎么连 Paseo

板子连 Paseo daemon 的方式和 Paseo App 添加 Host 时一样，有两种：

- **扫描二维码**，加密 relay 连接。板子扫 Paseo 在 设置 › 你的 Host › 配对设备 里出示的配对码，
  经 Paseo 的中继连过去，和 daemon 之间端到端加密。什么都不用暴露到公网。
- **直接连接**，本地网络或 VPN。在板子上填 Host、端口、是否用 SSL 和 daemon 密码。

两种方式都在 daemon 设了密码时用 **daemon 密码**登录，之后只调用 Farol 插件的接口
（`farol.next`、`farol.focus`、`farol.act`，协议见 [farol/1](protocol/README.md)）。
插件不开端口，也不保存密码。

> **板子把连接用的东西明文存在 flash（NVS）里**：daemon 密码，以及走中继时的配对码。
> 拿到板子和一根 USB 线的人就能读出来。密码能完全控制你的 daemon；daemon 没设密码时，
> 配对码也一样（Paseo 也提醒要像密码一样保管它）。只把 Farol 装在你自己保管的板子上，
> 给 daemon 设一个密码，板子丢了就改掉它。

从别的网络直接连接时，需要一个把 WebSocket 转发到 daemon 的 HTTPS 地址，
[docs/remote-access.md](docs/remote-access.md) 给了 Caddy、Cloudflare Tunnel、Tailscale Funnel
的写法。用 SSL 时证书必须是公开信任的。

## 需要什么

- 一块 ESP-Mosaico 板子。左槽的摄像头模块用来扫 Wi-Fi 码和 Paseo 的配对码；没有它就用直接连接，
  在屏幕上输入。
  右槽的交互模块可选：插上后，屏幕随人亮灭、跟着环境光调亮度，LED 显示灯的状态。
- Paseo daemon 0.14 或更新，并开启插件。扫码连接还要启用它的中继（第一次打开配对码时 Paseo 会问）。
- 一个 [ESP-Mosaico 工作区](https://github.com/esp-mosaico/esp-mosaico-vibe)（含子模块）、
  工作区固定提交的 ESP-IDF、Python 3.10+，以及 Rust stable 和
  `riscv32imafc-unknown-none-elf` 目标（`rust/rust-toolchain.toml` 会通过 rustup 装上）。

验证过的组合：工作区 `esp-mosaico-vibe` 1872fd8，ESP-IDF 7b9cc1ac，ESP-GSP 1.9.0
（gspc 0.10.0、模拟器 1.9.0），Paseo 0.14.0。

## 安装

1. 固件。把本仓库放在工作区的 `projects/farol`，在工作区根目录运行：

   ```sh
   python mosaico.py iris system-update --project projects/farol
   ```

   它会构建固件，并装在板子保留的 Vibe Mode 旁边。摄像头驱动会在配置时给 ESP-IDF 打补丁，
   所以 Farol 要用一份固定版本 ESP-IDF 的私有副本来构建；`$IDF_PATH` 没打补丁时构建会停下，
   以免改动共享的 ESP-IDF。按
   [components/app_camera/README.md](components/app_camera/README.md#building-with-the-camera)
   做一次副本，之后带上那三个环境变量运行上面的命令。

2. 插件。在运行 daemon 的 Mac 上：

   ```sh
   cd plugin && npm install && npm run check
   paseo plugin install "$PWD" --id farol
   ```

## 连接一块板子

1. Wi-Fi（2.4 GHz）。在板子的“没有连上 Wi-Fi”页上，扫手机 Wi-Fi 设置里出示的网络二维码，
   或者选一个网络、输入密码。
2. Paseo。在板子上选“扫描二维码”，扫 Paseo 在 设置 › 你的 Host › 配对设备 里出示的码；
   或者选“直接连接”，填 Host、端口（默认 6767）、是否用 SSL 和 daemon 密码。板子会先告诉你
   要连的是哪台 Paseo，点“连接”。daemon 设了密码而码里没带时，板子会再问你要。

## 怎么用

| 在哪 | 手势 | 结果 |
|---|---|---|
| 任何地方 | 左右滑 | 额度 ← 首页 → 卡片组 |
| 任何地方 | 从顶部下拉 | 控制中心：Wi-Fi、Paseo、勿扰、声音、振动、亮度 |
| 首页 | 轻点灯 | 跳到最紧急的那张卡 |
| 卡片组 | 上下滑 | 换下一个 agent，最紧急的排在前面 |
| 提问卡 | 按住一个选项（0.5 秒） | 用这个选项回答 |
| 工作中的卡 | 按住（1.2 秒） | 停止这一轮；提前松手就取消 |
| 完成的卡 | 点“已读” | 变灰，一小时后离开卡片组 |
| AI 键 | 单击 / 长按 | 去最紧急的卡，再按回到灯 / 勿扰一小时 |
| 板子 | 扣过来放 | 扣着的时候勿扰 |

轻点永远不会作用到 agent。插件设置里关掉“允许板子操作 agent”，板子就只能看不能动。
没插交互模块时，两分钟没碰屏幕会变暗，十分钟熄屏；插上后，有人在旁边就一直亮着，
人走开三分钟后熄屏。熄屏时碰一下屏幕只会把它唤醒。

## 开发

在本目录下：

```sh
cd rust && cargo test -p logic                 # 逻辑、协议、文字排版的测试
python tools/dev.py sim                        # 浏览器里的模拟器，跑演示数据
python tools/dev.py check                      # 脚本化场景检查（截图）
python tools/simdrive.py                       # 长按、键盘、各页面的交互流程
python tools/dev.py scene                      # 改了 tools/ui/gen_scene.py 之后
cd plugin && npm run check                     # 插件类型检查和测试
node tools/relay_vectors.mjs > protocol/fixtures/relay.json   # Paseo 改了中继协议之后
```

模拟器跑的是和板子上同一份 Rust 逻辑，数据来自内置的演示 agent。`plugin/dev/` 会起一个
隔离的 Paseo daemon 和脚本化的假 agent，像板子一样驱动插件，不碰你自己的 daemon。
打开 `CONFIG_FAROL_BENCH` 会构建一个回放脚本化手势、打印每帧耗时的测量固件
（[components/app_bench/README.md](components/app_bench/README.md)）。

## 目录

- `main`：只管启动顺序。
- `components/app_display`：CO5300 屏幕、presenter、触摸、亮度。
- `components/app_logic`：场景（`ui/main.json`，由 `tools/ui/gen_scene.py` 生成）、字体和 Rust 构建；
  `host/` 是模拟器后端。
- `components/app_net`、`app_link`：Wi-Fi，以及连 Paseo 的 WebSocket（直接连接或中继；
  中继的加密在 `rust/logic/src/relay.rs`）。
- `components/app_feedback`、`app_camera`：振动、声音、交互模块、传感器；摄像头和二维码识别。
- `components/app_bench`：帧测量工具（默认关闭）。
- `rust/logic`：板子上所有的判断，板子、模拟器和测试共用；`rust/platform`：板子这一侧；
  `rust/host`：模拟器这一侧。
- `plugin/`：Paseo 插件；`protocol/`：farol/1 协议和两边测试共用的样例。
- `tools/`：场景和字体生成器、模拟器驱动。

## 许可

Apache-2.0（[LICENSE](LICENSE)）。字体是 Noto Sans SC 和 Geist 的子集，
遵循 SIL Open Font License（`components/app_logic/ui/fonts/OFL-*.txt`）。
