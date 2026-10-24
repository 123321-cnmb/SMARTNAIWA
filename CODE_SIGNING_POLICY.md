# 代码签名策略（Code Signing Policy）

本文是奶蛙（naiwa-pet）面向用户公开的代码签名策略，也是向 SignPath Foundation
申请免费开源签名时需要提交的说明。

## 当前状态

截至本文件编写时，**项目尚未获得 SignPath Foundation 的免费签名**，
GitHub Release 上的安装包**没有** Authenticode 签名。

在签名开通之前，Windows SmartScreen 可能提示「Windows 已保护你的电脑」，
用户需要点「更多信息 → 仍要运行」才能继续。这是未签名程序的正常表现，
不代表程序被判定为恶意软件。

签名开通后，本文件会更新为实际状态，并注明第一个带签名的版本号。
在那之前，**不会**把任何产物描述为「已签名」或「由 SignPath 签名」。

## 角色与职责

| 角色 | 负责人 |
| --- | --- |
| 提交者 / 审阅者 | 拥有本仓库写权限的贡献者（naiwa-pet maintainers） |
| 签名审批者 | naiwa-pet maintainer，负责逐次手动审批签名请求 |

- 来自仓库外部的贡献需要由有写权限的成员审阅并合并。
- **每一次签名请求都需要人工审批**，不存在自动批量签名。
- GitHub 账号和 SignPath 账号都必须启用多因素认证（MFA）。

## 产物如何构建

- 发布的安装包**只能**由本仓库的 GitHub Actions 工作流（`.github/workflows/release.yml`）
  在 GitHub 托管的 `windows-latest` runner 上，从公开源码构建。
- 触发方式是推送 `v*` 格式的 git tag。
- 私有机器上本地构建的产物不具备签名资格。
- 每个版本的产品名、文件版本信息必须与 `package.json` / `tauri.conf.json` 一致。

## 签名覆盖范围

- 签名覆盖：奶蛙自己的 Windows 安装包（`奶蛙_<版本>_x64-setup.exe`）。
- **不覆盖**：第三方二进制依赖。它们保留各自的作者身份和声明，
  不会被当作奶蛙自有的代码签名。

## 隐私

- 奶蛙**不**向项目维护者发送任何分析数据、广告标识符、崩溃报告或用户文件。
- 配置、自选股列表等只保存在用户本机（`%APPDATA%\naiwa-pet\`）。
- 唯一的外发网络请求是用户主动触发时，向用户自己配置的模型服务
  （默认 MiniMax 开放平台）发送对话内容，以及查询公开的股票行情接口。
  这些请求受相应服务方自己的隐私政策约束。
- 打包应用本身**不内含任何 API Key**，用户必须自行配置。

## 验证方式

签名生效后，下载安装包并验证：

```powershell
Get-AuthenticodeSignature .\奶蛙_<版本>_x64-setup.exe | Format-List Status, SignerCertificate
```

预期 `Status` 为 `Valid`，签名者为 SignPath Foundation。
同时请核对同一 Release 里的 `SHA256SUMS.txt`。

## 致谢

免费代码签名由 SignPath.io 提供，证书由 SignPath Foundation 签发。
