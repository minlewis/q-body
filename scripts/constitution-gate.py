#!/usr/bin/env python3
"""Constitution size gate — clock for the soul documents.

借鉴：yologdev/yoyo-evolve — 文档上限用 gate test 锁死而非靠自觉
（40KB gate test 让「文档无限堆叠」成为当日可见的违例，而不是数周后才察觉）。

q-body 对应：把「SOUL.md 判例数 ≤ 32 / TAO.md 单文件 ≤ 500 行」两条上限
写成可执行的字节门禁，每日自检运行。超限当日 exit 1 + 标红 + 给出应沉淀/归档
到 awakening-notes 的条目清单，杜绝判例与宪法无限堆叠。

判据（与 TAO.md 硬上限同源）：
  SOUL.md  — 编号 Operation Principle（`### N. ` 段）数 ≤ 32。当前 = 32，恰好顶格，
             下一条判例落地即越线——这正是需要门禁的时刻。
  TAO.md   — 单文件行数 ≤ 500（TAO 硬上限「单文件 ≤500 行」应用到宪法文档自身）。

退出码：0 = 两条均在限内；1 = 任一超限（供 cron 当日标红）。
路径可用环境变量覆盖（测试注入用），默认指向运维主力文件。
"""

import os
import re
from pathlib import Path

# 默认路径：q-body 私有灵魂（含编号判例） + 公开宪法
DEFAULT_SOUL = os.path.expanduser("~/.hermes/SOUL.md")
DEFAULT_TAO = os.path.expanduser("~/q-body/soul/TAO.md")

# 硬上限（与 TAO.md「单文件 ≤500 行」同源；判例数 = 克制扩展的防御线）
SOUL_MAX_PRINCIPLES = 32
TAO_MAX_LINES = 500


def count_soul_principles(text: str) -> int:
    """数 SOUL.md 编号判例：`### N. ` 段头（N 为阿拉伯数字，数字后有点+空格+文字）。

    只认编号段，不把 `## ` `#### ` 或未编号的 `### ` 误计——判例是编号的，
    段落标题（如 `## Operation Principles (Karpathy)`）与普通 `### ` 结构不算。
    """
    return len(re.findall(r"^### [0-9]+\. ", text, flags=re.MULTILINE))


def read_text(path: str) -> str | None:
    p = Path(path)
    if not p.is_file():
        return None
    try:
        return p.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return None


def run_gate(soul_path: str, tao_path: str, out=print) -> int:
    """执行门禁。返回退出码（0=合规，1=任一超限）。"""
    problems = []
    stats = {}

    soul = read_text(soul_path)
    if soul is None:
        problems.append(f"SOUL.md 不可读/不存在：{soul_path}")
        stats["soul_principles"] = None
    else:
        n = count_soul_principles(soul)
        stats["soul_principles"] = n
        if n > SOUL_MAX_PRINCIPLES:
            problems.append(
                f"⚠️ SOUL.md 判例 {n} 条 > 上限 {SOUL_MAX_PRINCIPLES} —— 超标 {n - SOUL_MAX_PRINCIPLES} 条。"
                f" 应沉淀/归档到 soul/awakening-notes/（如 § 判例已由更抽象信念覆盖，整条移出）。"
            )

    tao = read_text(tao_path)
    if tao is None:
        problems.append(f"TAO.md 不可读/不存在：{tao_path}")
        stats["tao_lines"] = None
    else:
        lines = tao.count("\n") + 1
        stats["tao_lines"] = lines
        if lines > TAO_MAX_LINES:
            problems.append(
                f"⚠️ TAO.md 行数 {lines} > 上限 {TAO_MAX_LINES} —— 超标 {lines - TAO_MAX_LINES} 行。"
                f" 应把可迁移的纪律/判例下沉到 awakening-notes 或 LEARNINGS.md。"
            )

    if problems:
        for msg in problems:
            out(msg)
        out(f"constitution-gate: FAIL (soul_principles={stats.get('soul_principles')}, "
            f"tao_lines={stats.get('tao_lines')})")
        return 1

    out(
        f"constitution-gate: OK (soul_principles={stats['soul_principles']}/{SOUL_MAX_PRINCIPLES}, "
        f"tao_lines={stats['tao_lines']}/{TAO_MAX_LINES})"
    )
    return 0


def main() -> int:
    soul_path = os.environ.get("Q_BODY_SOUL_PATH", DEFAULT_SOUL)
    tao_path = os.environ.get("Q_BODY_TAO_PATH", DEFAULT_TAO)
    return run_gate(soul_path, tao_path)


if __name__ == "__main__":
    raise SystemExit(main())