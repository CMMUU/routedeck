# GitHub 与 Gitee 同步

GitHub `CMMUU/serylane` 是 Serylane 代码和正式发行版的来源，Gitee `cmmuu/serylane` 保存国内副本。自 2026-09-04 起项目采用 GPL-3.0-only 开源，两站都应设为公开。先在 GitHub 完成测试、提交和发版，`Sync GitHub to Gitee` 工作流再复制全部分支、标签及最新正式版的原始附件。Gitee 同步不重新编译安装包。

项目展示名为 `Serylane`，两站仓库路径统一为 `serylane`。同步器按新仓库名进行精确目标校验，维护者应先完成两站仓库更名并核对 Git remotes，再运行写入同步。仓库更名不意味着新版本已经发布或同步成功。GitHub 历史版本的标题、附件、签名与校验值保持不变；Gitee 历史附件按下述保留策略清理。

## 国内镜像保留策略

实际路径迁移使用 `--migrate-legacy-path`，仅允许 GitHub 仓库 ID `1355770287` 与 Gitee 原仓库 ID `50078322`。先验证目标地址未被其他仓库占用、账号与可见性，再通过官方 PATCH 的 `name/path` 字段更名；更名后核对仓库 ID、主分支提交和全部 Release ID/标签不变。已迁移时只读验证，不重复写入。不会新建、删除或覆盖仓库。

新版应用、官网和同步器使用 `cmmuu/serylane`。历史签名安装包及旧清单不重写，旧版客户端如遇国内旧 API 失效，可经 GitHub 备用渠道升级；不依赖 Gitee 为所有旧 API 提供重定向。

2026-09-06 经维护者确认，Gitee **仅保留最新 1 个正式版本的镜像附件**；GitHub 完整历史、两站源码与 Git 标签，以及已有 Gitee Release 元数据不删除。工作流显式传入 `--keep-latest-releases 1`，本地脚本不传此参数仍采用不删除历史的旧行为。

- 按 `v主版本.次版本.修订版本` 的数字顺序选取最新正式版，不把草稿、预览版或重新编辑的旧版当作最新版。定时补偿或旧 Release 的编辑事件不会重新上传已淘汰的附件。
- 清理对象只包括同仓库、同正式版本、同文件名且大小一致的 GitHub 原始附件副本。目标独有的版本、预览版或附件不会删除，并继续计入容量预算；有冲突或容量仍不足就停止。
- 先下载并校验全部待保留的新包，再下载 GitHub 历史原件和 Gitee 待清理附件，逐个比较 SHA-256。任何备份缺失、内容不同、重复 ID/名称或源发布发生变化，均停止清理。
- 优先完整上传并验证新版本，然后清理旧包。若新旧包共存超出容量，先清理腾出空间所必需的最旧版本，其余旧包在新版本完成后再清理。必要的提前清理完成后若上传失败，已清理文件仍可从 GitHub 恢复；不承诺事务性回滚。
- 只调用单个 Release 附件的删除接口；删除 Release、仓库和标签的 API 路径被禁止。每次删除后读回确认，结果不明确时不盲目重试。恢复旧版本应从 GitHub 获取原包，不能绕过保留策略把全部历史重新灌回 Gitee。
- 手动工作流默认 `dry_run=true`，先列出保留版本、待清理版本、目标 Release ID 和预计字节数，不推送或删除任何远端数据。确认清单后以 `dry_run=false` 执行；自动事件按已批准的保留规则执行。
- 同步使用 GitHub 托管的 Ubuntu ARM 运行器，手动入口保留 Ubuntu x64 和 macOS 14 选项用于排查停滞的云端网络连接；不在维护者的活动电脑上运行上传或安装。运行器只接受这三个固定官方标签，身份、配额与校验规则完全相同。日志分别显示源文件核验、上传、目标下载回验与清理阶段，不记录令牌或签名下载 URL。
- 附件通过系统 curl 流式上传，使用与其他 API 写入一致的 Bearer 请求头和 formData 令牌。配置从标准输入传递，令牌不进入命令行、URL 或日志；不读取用户 curlrc、不自动跟随跳转、不自动重试 POST，保留 TLS 验证并正常协商 HTTP 协议。连接上限 30 秒、单次完整上传上限 1800 秒；连续 120 秒低于 32 KiB/s 时提前结束，避免极慢连接消耗整个工作流。日志只记录文件名、HTTP 状态、发送字节、耗时和速率；超时或响应不确定时仍先查询并核验现有附件，绝不盲目再传。
- 手动入口可选择单传输执行，用于排查并发与网络拥塞；常规云端仍最多三个并发，完整容量预检和最终更新清单屏障保持不变。
- 云端最多同时传输 3 个普通附件，每个任务使用独立 HTTP 客户端；完整容量预检仍在所有传输之前进行。任何文件失败后停止分配新文件，等待已在途请求收尾，不盲目重试；全部新旧更新清单仍串行放在最后，只有全部安装包、签名和校验文件验证通过才发布。慢速跨境传输的工作流上限为 180 分钟。本地默认串行，显式传入 `--transfer-workers 3` 才开启并发。

## 自动触发

- GitHub 任意分支或标签 push。
- GitHub Release 发布或编辑。
- 同仓库 `Release bundles` 工作流成功完成；只接受 push 或手动运行，拒绝 fork 和 PR 工作流来源。
- 每天 UTC 02:17（北京时间 10:17）进行补偿同步，补齐附件后续上传、事件遗漏和暂时网络故障。
- Actions 页面手动运行 `Sync GitHub to Gitee`。

push 只同步分支与标签；发布成功、Release 事件、定时补偿和手动执行会检查最新版及全部目标附件的容量和保留策略。并发组只允许一个运行中的同步。工作流始终从可信 `main` 检出同步脚本，不执行上游构建产物或上游工作流提交中的脚本。

由 `GITHUB_TOKEN` 创建的 Release 通常不会再次触发另一个 `release` 工作流，因此保留成功构建后的 `workflow_run` 和每天的补偿入口。Release 应在完整附件上传后发布；单独上传附件不保证产生 `release: edited` 事件。不要同时启用会改动相同分支的反向镜像。

## 首次配置

1. 创建 Gitee 目标仓库，将 GitHub 与 Gitee 都设为公开，默认分支设为 `main`。空仓库不要额外初始化 README，以免引入分叉历史。
2. 在 GitHub 仓库的 Actions Secrets 配置 `GITEE_TOKEN`，赋予目标仓库 Git 推送、Release/附件写入及 `/user` 读取所需权限。身份必须是 `cmmuu`。GitHub 侧使用仅 `contents: read` 的内置 `GITHUB_TOKEN`。
3. 手动运行同步工作流，查看引用校验与每个版本的附件验证结果。离线测试通过不等同于远端同步成功。

脚本通过进程环境和 Git credential helper 的专用管道提供凭据，不把令牌写进远端 URL、命令行参数、Git 配置或日志。Gitee GET/DELETE 使用 Bearer；POST/PATCH 依照 API 的 form 字段在内存请求体发送凭据。

以下 Actions Variables 可按已核实的目标配置填写；留空使用默认值：

| 变量 | 默认值 | 用途 |
| --- | --- | --- |
| `GITEE_ASSET_HOSTS` | 空 | 在内置 `foruda.gitee.com` 之外追加经核实的附件下载 CDN 精确主机名，逗号分隔，不支持通配符 |
| `GITEE_MAX_ASSET_BYTES` | `104857600` | 单附件本地预检按 100 MiB 解释；以 Gitee 服务端最终校验为准 |
| `GITEE_MAX_TOTAL_BYTES` | `1000000000` | 单仓库附件总上限，按社区版 1 GB 保守解释 |
| `GITEE_OTHER_ATTACHMENT_BYTES` | `0` | 普通仓库附件已占用或预留的字节数；新空仓库可使用零 |

Gitee 官方配额同时统计普通仓库附件与 Release 附件。同步器会枚举**所有 Gitee Release** 的已有附件，加上待保留版本缺少的源附件和上述预留容量，并计算经校验可清理的旧镜像；同名已有附件不重复计数。Release API 无法列出普通仓库附件，因此维护者在增加普通附件后应更新预留值。预检不能代替服务端最终额度检查。不要在服务端拒绝后继续提高上限重试；更高套餐的额度应先核实，脚本单附件技术上限为 512 MiB。

单附件和总量是两个独立限制。v0.7.2 的 amd64 AppImage 为 `100235768` 字节（约 95.59 MiB），略高于原脚本的十进制 `100000000` 预检值。本次按 100 MiB 允许尝试上传，实际能否接收仍由 Gitee 验证；不拆包、不重签名、不修改已发布校验清单。清理历史本身不会提高单文件上限。

## 校验与失败恢复

- 在任何远端写入前检查身份与可见性；每次写入前再次核实仓库隐私。私有 GitHub 源不能同步到公开 Gitee 目标。
- 分支和标签仅使用原子、非强制推送，并读回校验 object ID。不删除目标多余引用、不强推分叉历史、不覆盖冲突标签。
- 先同步并校验代码引用，再检查待保留附件的单文件大小、全部目标附件及清理前后预计总量。容量无法满足时会阻止 Release 写入与清理，但已同步的代码与标签保留；工作流仍报告失败，不能将此状态视为发版同步完成。
- 保留 Release 标题、正文与预发布标志；草稿跳过。写入后读回核对元数据。
- 原始附件先验证 GitHub 摘要；若含 `SHA256SUMS.txt`，它必须完整匹配该版全部附件。旧 GitHub 附件没有 digest 时，每次从 GitHub 重新下载，复用缓存时比较新旧内容，不能仅凭本地摘要信任缓存。
- 同名 Gitee 附件必须下载回验大小及 SHA-256。一致则复用，缺失才上传，冲突或重复则停止。目标独有的版本、附件和引用保留，不擅自清理。
- GET 请求的临时错误最多尝试三次。POST/PATCH/DELETE 不盲目重试；上传结果不明时停止，下一次同步先查询已有附件，确认已上传就校验复用。删除结果不明时先读回确认。可从 Actions 重跑失败任务，也会在下一次事件或每天补偿时再检查。
- 下载只接受 HTTPS 及获准的精确域名；发生跳转后不继续发送 Authorization，不记录签名 URL。未知 CDN 域名会导致同步停止，应核实来源后再配置。

Gitee Release API 没有与 GitHub 草稿等价的上传阶段，因此新版本的元数据创建后、全部附件回验前，页面可能短暂显示部分附件。更新清单最后上传，以同步工作流成功为该版本复制完整的依据。失败不会删除 GitHub 版本；已经过保留策略清理的 Gitee 历史包不会自动回滚。

## 本地运行

环境需要 Python 3.10+、Git 与 curl（GitHub 托管运行器已预装；测试使用 Python 3.12）。通过安全的进程环境注入 `GITHUB_TOKEN`、`GITEE_TOKEN`，不要在命令行中填写令牌。

```sh
python3 scripts/sync_gitee.py --repo serylane --scope all --keep-latest-releases 1 --work-dir /private/path/gitee-sync
python3 scripts/sync_gitee.py --repo serylane --scope all --keep-latest-releases 1 --work-dir /private/path/gitee-sync --apply
```

第一条仅进行身份、隐私和容量预检；第二条允许写入已经核实的目标。缓存可能包含私有代码和安装包，应使用私有目录。发现缓存或目标同名内容冲突时不要强行覆盖，应查明源资产是否被替换。

```sh
python3 -m unittest discover -s scripts -p 'test_*gitee*.py' -v
```

离线测试不使用真实账号或凭据，覆盖隐私拒绝、引用范围、重定向脱敏、摘要、容量、无 digest 缓存、幂等、不确定上传恢复，以及显式版本保留、备份校验、分阶段清理、204 删除响应、并发源变化、受保护附件和不确定删除恢复。

## 官方依据

- [Gitee API v5 规范](https://gitee.com/api/v5/doc_json)：按 5.4.92 核对 Release 创建/修改、附件列表/上传/下载与单附件 DELETE（204）；`AttachFile` 有 id/name/size，没有摘要字段。
- [Gitee 社区版配额](https://help.gitee.com/account/usage-quota)与[发行版创建及附件容量](https://help.gitee.com/repository/release/create)。
- [GitHub Actions 并发](https://docs.github.com/en/actions/concepts/workflows-and-actions/concurrency)与[工作流触发规则](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/trigger-a-workflow)。
