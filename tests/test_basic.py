"""
Q-Body 测试套件 - 基础运行测试
"""

from core import __version__


def test_version():
    """验证核心模块版本存在"""
    assert __version__ == "0.0.1"


def test_main_runs():
    """验证入口可以运行"""
    from main import main
    # 至少不报错
    assert callable(main)