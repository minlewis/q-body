#!/usr/bin/env python3
"""Vacuous test detector — 扫描 Rust 测试函数，标记无断言的"空转测试"。

借鉴：yologdev/yoyo-evolve — "Fix vacuous context tests that silently pass
without asserting in CI"

用法：
    python3 scripts/check_vacuous_tests.py [src_dir]

退出码：
    0 — 所有测试函数都含断言（或无测试函数）
    1 — 发现 vacuous test（CI 拒绝合入）
"""

import os
import re
import sys

# 断言标记：函数体中出现任一即视为有断言
ASSERTION_PATTERNS = [
    "assert!",
    "assert_eq!",
    "assert_ne!",
    "should_panic",
    "panic!",  # 显式 panic 也算主动断言失败
]

# 匹配 #[test] 或 #[tokio::test] 标注的函数
TEST_FN_RE = re.compile(
    r"#\[(?:tokio::)?test\]\s*(?:async\s+)?fn\s+(\w+)",
    re.MULTILINE,
)


def find_rust_files(src_dir: str) -> list[str]:
    """递归查找所有 .rs 文件（排除 target/ 目录）。"""
    rust_files = []
    for root, dirs, files in os.walk(src_dir):
        # 跳过 target 目录
        if "target" in dirs:
            dirs.remove("target")
        for f in files:
            if f.endswith(".rs"):
                rust_files.append(os.path.join(root, f))
    return rust_files


def extract_test_functions(content: str) -> list[tuple[str, str]]:
    """提取测试函数名及其函数体。

    返回 [(fn_name, body_text), ...]
    """
    results = []
    for match in TEST_FN_RE.finditer(content):
        fn_name = match.group(1)
        # 从函数名后开始，找到 { ... } 函数体
        body_start = content.find("{", match.end())
        if body_start == -1:
            continue

        # 简单花括号匹配
        depth = 0
        body_end = body_start
        for i in range(body_start, len(content)):
            if content[i] == "{":
                depth += 1
            elif content[i] == "}":
                depth -= 1
                if depth == 0:
                    body_end = i + 1
                    break

        body = content[body_start:body_end]
        results.append((fn_name, body))
    return results


def is_vacuous(body: str) -> bool:
    """检查函数体是否包含至少一个断言。"""
    return not any(pat in body for pat in ASSERTION_PATTERNS)


def main() -> int:
    src_dir = sys.argv[1] if len(sys.argv) > 1 else "src"
    if not os.path.isdir(src_dir):
        # 没有源码目录，无测试可检查
        print(f"[vacuous-test] 目录 {src_dir} 不存在，跳过")
        return 0

    rust_files = find_rust_files(src_dir)
    vacuous: list[tuple[str, str]] = []  # (file, fn_name)

    for filepath in rust_files:
        with open(filepath, encoding="utf-8") as f:
            content = f.read()
        for fn_name, body in extract_test_functions(content):
            if is_vacuous(body):
                vacuous.append((filepath, fn_name))

    total_tests = sum(
        len(extract_test_functions(open(fp, encoding="utf-8").read()))
        for fp in rust_files
    )

    if vacuous:
        print(f"[vacuous-test] ❌ 发现 {len(vacuous)} 个空转测试（无断言）：")
        for filepath, fn_name in vacuous:
            print(f"  {filepath} :: {fn_name}")
        print(f"\n共扫描 {total_tests} 个测试函数，{len(vacuous)} 个无断言。")
        print("CI 拒绝合入：请为每个测试函数添加 assert! / assert_eq! / assert_ne!。")
        return 1

    print(f"[vacuous-test] ✅ 扫描 {total_tests} 个测试函数，0 个空转测试。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
