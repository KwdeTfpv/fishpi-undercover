# 谁是卧底游戏后端

## 项目简介
本项目为"谁是卧底"游戏的后端服务，基于 Rust 语言开发，支持多种前端集成方式，适合自部署和二次开发。

## 主要功能
- 游戏房间管理
- 玩家身份分配与回合控制
- 游戏流程自动推进
- API 文档详尽（见 API.md）

## 快速开始
1. 克隆仓库：
   ```bash
   git clone <your-repo-url>
   ```
2. 进入项目目录，安装 Rust 环境。
3. 复制配置文件模板并根据实际情况修改：
   ```bash
   cp config.example.toml config.toml
   ```
4. 编译并运行：
   ```bash
   cargo run --release --bin fishpi-undercover
   ```

## 配置说明
- `config.toml`：主配置文件，包含端口、数据库等信息。
- 示例配置文件已提供（*.example.*），请勿上传真实配置。

## 注意事项
- 前端可通过 API 进行集成，详见 `API.md`。

## 目录结构
- `src/`：核心后端代码
- `data/`：数据文件
- `docs/`：文档与流程图
- `index.html`：示例前端页面

---
如有问题欢迎提 Issue 或 PR！ 