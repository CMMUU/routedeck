# Serylane 官网

基于已确认的三段磨砂玻璃设计实现，原生 HTML / CSS / JavaScript，无框架、第三方字体、分析脚本或客户端构建依赖。Cloudflare Workers Static Assets 托管；只上传 `public/`，不上传桌面程序源码、用户配置或密钥。

目标：`https://serylane.cmmuu.com/`。部署是否成功以实际 HTTPS 验收为准，源码中的域名与配置本身不代表已经上线。

## 本地检查

需要 Node.js 24、pnpm 11.19.0。

```sh
cd website
pnpm install --frozen-lockfile
pnpm test
pnpm dev
```

开发服务只监听 `127.0.0.1:4177`。它不会打开浏览器，也不会调用桌面客户端、本地代理或 Codex。可设置本进程的 `SERYLANE_PREVIEW_PORT` 改用其他空闲端口。

Cloudflare 的真实资源路由与安全头预览：

```sh
pnpm exec wrangler dev --local --ip 127.0.0.1 --port 4178 --show-interactive-dev-session=false
pnpm deploy:check
```

只需网页测试，无须启动、退出或切换桌面代理软件。`scripts/serve.mjs` 是便于审阅的静态开发服务；生产行为以 Wrangler / Cloudflare 为准。

## 上线

1. `pnpm exec wrangler whoami --json` 检查账号。尚未登录时，可执行 `pnpm exec wrangler login --device --browser=false`，由账号所有者在官方页面授权；不要在聊天、Git 或站点文件中放入 Token。
2. 在 Cloudflare 中确认 `cmmuu.com` 为正确账号的有效 zone。核对 `serylane.cmmuu.com` 的**准确 DNS 记录、Workers 自定义域名绑定和现有用途**。公共 DNS 返回 Cloudflare 地址也可能来自泛解析，不代表该子域名空闲。
3. 确认不会覆盖其他站点后，运行 `pnpm deploy`。配置只包含该子域名，不修改根域名、其他网站、账户安全设置或通配符 DNS。
4. 验收主页与四篇文档 HTTPS 200，未知路径 404、`.html` 别名归一化、robots、sitemap、canonical 和下载链接。成功后才更新仓库 About 的 Homepage、README 的部署状态。

CI 仅检查，不存储 Cloudflare 凭据，也不会自动登录或部署。不得在缺少授权时把 `deploy --dry-run` 当成上线成功。

## 下载快照与更新

当前固定为 **2026-09-08 核验的 v0.7.5**。GitHub 已正式发布，六个主包均返回 HEAD 200，Content-Length 与 Release API 的资产大小一致。国内镜像仍在同步；截至 **2026-09-08 11:39（UTC+8）** 的精确链接检查，仅 Linux ARM64 AppImage 返回 HEAD 200 且大小与 GitHub 一致，其余五个所选主包返回 404，不能宣称国内镜像齐全。这是下载可用性与元数据核验，不代表在本次检查中下载大包执行了独立哈希校验。

| 主包文件名 | GitHub 大小（字节） | 本次 Gitee HEAD |
| --- | ---: | --- |
| `RouteDeck_0.7.5_x64-setup.exe` | 17,908,356 | 404，未就绪 |
| `RouteDeck_0.7.5_arm64-setup.exe` | 15,140,650 | 404，未就绪 |
| `RouteDeck_0.7.5_x64.dmg` | 27,482,395 | 404，未就绪 |
| `RouteDeck_0.7.5_aarch64.dmg` | 25,154,497 | 404，未就绪 |
| `RouteDeck_0.7.5_amd64.AppImage` | 100,559,352 | 404，未就绪 |
| `RouteDeck_0.7.5_aarch64.AppImage` | 97,098,248 | 200，大小一致 |

有同版本、同架构、同格式的国内主包时，将国内直链置为主要操作；其余情况明确提示缺失，主要操作改为已核验 GitHub 包。不会静默换成另一个版本或格式。

当前仅 Linux ARM64 AppImage 为国内主操作、GitHub 备用；其余选项主操作为 GitHub，并明确提示当前所选国内包暂缺、保留国内发布页入口。无 JavaScript 时保留已核验的 Windows x64 GitHub EXE 直链，禁用无法工作的系统与架构选择器。这里是人工核验快照，不会自动探测镜像后续上传进度；更新国内可用集合前仍须复核具体链接。离线测试另外在 VM 内模拟国内包全部缺失或仅部分可用，持续验证格式提示、GitHub 回退和主操作 DOM 顺序，不向生产代码增加测试接口。

新版本发布后，必须先核对 GitHub Release、Gitee API、具体文件的 HEAD 状态及大小，再同步修改 `public/site.js` 的 release 快照、首页无 JavaScript 降级链接/版本文字和 `scripts/test-downloads.mjs` 的独立预期值。未发布的源码版本、标签或存在的签名文件，不是包已存在的证据。国内链接不齐时不谎报镜像成功。

桌面展示名为 Serylane，但 `RouteDeck_…` 安装包、仓库 URL、更新清单及签名身份保留以兼容旧客户端。

## SEO 与安全

- 首页和四篇文档拥有独立标题、描述与精确 canonical；真实语义 HTML 可直接读取，不依赖 JavaScript 生成正文。
- 包含 robots、sitemap、Open Graph、Twitter 摘要卡及 SoftwareApplication JSON-LD，无虚构评分、下载量或稳定性指标。
- 404 返回真实错误状态并使用 noindex；`.html` 文档路径由 Cloudflare 归一化到无扩展 canonical。
- 严格 CSP：不连接外部 API 或本地代理，不执行内联脚本或第三方脚本；网站没有登录、上传、支付或接管程序功能。
- 正文即时重新验证缓存，PNG 标志缓存一天；不对未带内容哈希的文件设置 immutable。
- 可被抓取不等于已被 Google 收录。上线后仍需域名所有者在 Search Console 验证站点并提交 sitemap；不得声称已提交或保证排名。

## 设计与素材

延续批准稿的冰白底色、蓝紫环境光、磨砂玻璃、深色中文、统一线性图标与细圆角滚动条。下载版本/渠道提示及准确安全文案是相对概念稿的有意调整。页面中的路由界面是原生代码构建的**只读示意**，不代表实时连接状态。

`public/assets/serylane-mark.png` 是仓库 `assets/brand/serylane-icon.png` 经 Tauri 官方图标转换生成的 256×256 PNG；`public/favicon.png` 为同源 32×32 PNG。新品牌源图来自内置 image_gen 对批准标志的提取，完整来源及提示词见 `assets/品牌素材来源.md`。没有使用整张设计图充当网页，也没有把生成资产留在外部临时路径供生产依赖。
