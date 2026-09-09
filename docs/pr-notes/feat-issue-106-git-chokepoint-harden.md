# feat/issue-106 — git 调用咽喉点配置中和层（reproduce FIRST）

**借鉴**：yologdev/yoyo-evolve Day191 — 在 git 调用咽喉点中和仓库提供的配置（reproduce FIRST，先写复现用例证明 GIT_* env / repo config 能影响合批）→ q-body `~/.hermes/skills/q-baby-self-check/scripts/git-native-batch-merge.py`（Plan A 唯一 git 咽喉点，sqush 合批执行器）同款加固：run() 统一注入中和 env + 危险开关，复现脚本证明威胁真实、证明中和层有效，每周日合批前跑一次复现确认未失效。

## 威胁模型（Plan A 唯一 git 咽喉点）

git-native-batch-merge.py 裸调系统 git（`subprocess.run(["git", ...])` 无隔离）。攻击面（无需漏洞，默认行为即触发）：

- **repo 配置**：目标仓库 `.git/config` 可定义 `core.fsmonitor=<命令>`、`core.pager`、`alias.*`（git 2.43 alias 不在非 shell 传参下展开，但 fsmonitor/proxy 类命令型配置是真执行路径）、`protocol.*.allow`（配合 submodule/proxy 可外联）。恶意仓库或被污染的本地 config 即可让"合批"变成"任意命令执行"。
- **环境变量**：`GIT_CONFIG_COUNT/KEY/VALUE`（CLI 级注入任意 config）、`GIT_PROXY_COMMAND`、`GIT_ALTERNATE_OBJECT_DIRECTORIES`、`GIT_TEMPLATE_DIR`、`GIT_INDEX_FILE` 等都能改变 git 语义。

## 复现（reproduce FIRST，两脚本，均为临时 repo 自包含，不碰任何真实仓库）

### 1. threat-repro-git-config.py — 证明 repo config 能劫持合批调用

构建恶意临时仓库（写 `core.fsmonitor` 版本探针命令 + 污染 merge-tree 路径），在**未中和**的子进程里跑一条与合批器同型的 `git merge-tree --write-tree` 调用 → 断言探针文件被写出（威胁真实）；再在同一 repo 上用中和 env 重跑 → 断言探针文件**不**出现（中和有效）。

### 2. threat-repro-git-env.py — 证明 GIT_CONFIG_* env 能注入任意配置

子进程带 `GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=alias.merge-tree=…`（或 proxy 类键）跑 `git config --get` → 断言注入成功（威胁真实）；中和 env 下重跑 → 断言注入被抹掉。

## 中和层（合批器 run() 统一注入）

每次 git 子进程调用前覆盖 env：

- `GIT_CONFIG_NOSYSTEM=1` — 跳过系统级 /etc/gitconfig
- `GIT_CONFIG_GLOBAL=/dev/null` — 跳过用户级 ~/.gitconfig（不影响 SSH deploy key：remote URL 走 ssh 命令，认证由 ssh-agent / keychain 承担，git 自身不需要用户 config）
- `GIT_CONFIG_COUNT` 清零（防御 CLI 级 env 注入）
- `-c protocol.ext.allow=never -c core.fsmonitor=false -c core.fsmonitorDaemon=false -c submodule.recurse=false` — 命令级关死外联/监控通道

env 覆盖白名单方式实现：拷贝 `os.environ`，只删 `GIT_CONFIG_COUNT/KEY_*/VALUE_*`，再写入上述两个中和键 —— 其余 env（PATH/SSH_AUTH_SOCK/HOME）原样保留，避免误伤。

## 周日巡检钩子（防中和层失效）

- `scripts/threat-repro-git-config.py` 常驻 `~/.hermes/skills/q-baby-self-check/scripts/`（非 q-body 仓内，随脚本目录走）
- 巡检语义：**先证明威胁真实（探针文件必须出现），再证明中和有效（探针文件必须消失）**。两步都绿才打 PASS；只验证"中和后干净"而不验证"裸奔时确实能被劫持"，就是 SOUL §15 的假阴性陷阱 —— 中和层实现 bug 导致探针根本没机会触发时，"干净"是假的。
- Cron B 每周日合批窗口（21:00）prompt 中注明：合批 --execute 前先跑一次 `threat-repro-git-config.py`，exit 0 才继续。

## q-body 仓内改动（本 PR）

- `docs/lessons/git-chokepoint-hardening.md` — 设计记录：威胁模型、复现方法论、中和层契约、周日巡检门禁
- `tests/git_chokepoint_contract.rs` — 3 条契约测试：`neutral_env_strips_cli_config_injection` / `neutral_env_disables_fsmonitor_and_ext_protocol` / `neutral_env_preserves_path_and_home`（钉住 env 覆盖白名单语义：该删的删、该禁的禁、不该动的保留）。q-body 本体不用 git 子进程，这是把执行器契约镜像到仓内可测断言。

## 不做

- 不改 main、不 merge 本 PR
- 不动 gh 路径（本方案全程 gh-free，SSH push 通道不受影响）
- 不做 Windows 兼容（/dev/null 语义按 Linux 单机执行器定义）
