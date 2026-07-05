#!/usr/bin/env python3
import argparse
import ctypes
import glob
import json
import locale
import os
import plistlib
import re
import shutil
import socket
import subprocess
import sys
import threading
import time
import uuid
import shlex
import traceback
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path
from tkinter import BOTH, END, LEFT, RIGHT, X, Y, BooleanVar, Button, Canvas, Checkbutton, Entry, Frame, Label, Scrollbar, StringVar, Text, Tk, Toplevel, filedialog, messagebox
from tkinter import font as tkfont
from tkinter import ttk

from . import VERSION
from . import core as rsshmount
from .notices import THIRD_PARTY_NOTICES
from .paths import user_data_dir
from .rclone import augment_process_path, install_managed_rclone, managed_rclone_path, manual_install_text
from .updates import check_for_updates, format_update_info


APP_TITLE = "SSH MountMate"
CACHE_SIZE_CHOICES = ["default (off)", "1G", "5G", "10G", "20G", "50G", "100G", "500G"]
CACHE_AGE_CHOICES = ["default (1h0m0s)", "5m", "15m", "30m", "1h", "6h", "24h", "168h"]
MIN_FREE_CHOICES = ["default (off)", "1G", "5G", "10G", "20G", "50G", "100G"]
WRITE_BACK_CHOICES = ["default (5s)", "0s", "5s", "10s", "30s", "1m", "5m"]
DIR_CACHE_TIME_CHOICES = ["default (5m0s)", "30s", "1m", "5m", "15m", "1h"]
BUFFER_SIZE_CHOICES = ["default (16Mi)", "0", "8Mi", "16Mi", "32Mi", "64Mi", "128Mi"]
LANGUAGE_CHOICES = {"auto": "Auto", "en": "English", "zh": "中文"}
FONT_FAMILY_EN = "Segoe UI"
FONT_FAMILY_ZH = "Noto Sans CJK SC"
RCLONE_CONFIG_LOCK = threading.RLock()
DEFAULT_MOUNT_ALL_WORKERS = 4
DEFAULT_UNMOUNT_ALL_WORKERS = 8
BATCH_WORKER_CHOICES = ["1", "2", "3", "4", "6", "8", "10", "12"]
HOME_MOUNTPOINT_VALUE = "__home_mnt__"
FORM_LABEL_CHARS = 22
MACOS_STARTUP_HELPER_NAME = "SSHMountMateMountHelper"
TEXT = {
    "en": {
        "ready": "Ready",
        "loading_configs": "Loading configs...",
        "no_configs": "No configs yet.",
        "settings": "Settings",
        "add_config": "Add config",
        "refresh": "Refresh",
        "mount_all": "Mount all",
        "unmount_all": "Unmount all",
        "mount_all_started": "Mounting {count} configs...",
        "unmount_all_started": "Unmounting {count} configs...",
        "batch_complete": "Batch operation complete. {done}/{count} changed.",
        "batch_busy": "A batch operation is already running.",
        "operation_busy": "This config is already being processed.",
        "mount_started": "Mounting {name}...",
        "unmount_started": "Unmounting {name}...",
        "checking_deps": "Checking dependencies...",
        "check_dependencies": "Check dependencies",
        "install_missing_dependencies": "Install missing dependencies",
        "check_updates": "Check for updates",
        "checking_updates": "Checking for updates...",
        "update_check_failed": "Update check failed",
        "view_mount_logs": "View mount logs",
        "view_licenses": "View licenses",
        "missing_dependencies": "Missing dependencies: {items}. Install or show instructions now?",
        "deps_status": "rclone: {rclone}    {mount_dep}: {mount}    ssh: {ssh}",
        "ok": "ok",
        "missing": "missing",
        "cache_root": "Cache root",
        "vfs_cache_mode": "VFS cache mode",
        "max_cache_size": "Max cache size",
        "max_cache_age": "Max cache age",
        "min_free_space": "Min free space",
        "write_back": "Write-back delay",
        "dir_cache_time": "Directory cache",
        "buffer_size": "Buffer size",
        "mount_workers": "Mount concurrency",
        "unmount_workers": "Unmount concurrency",
        "language_help": "Auto uses Chinese on Chinese systems and English otherwise.",
        "cache_root_help": "Local folder used by rclone VFS cache. Put it on a fast disk with enough free space.",
        "vfs_cache_mode_help": "Controls local file caching. Higher modes improve app compatibility but use more disk.",
        "max_cache_size_help": "Upper limit for VFS cache size. Default means rclone does not enforce this limit.",
        "max_cache_age_help": "How long cached objects may stay before rclone can evict them. Default is 1 hour.",
        "min_free_space_help": "Keep this much local disk space free for other applications.",
        "write_back_help": "Delay before changed files are written back to the server. Longer delays can smooth frequent small writes.",
        "dir_cache_time_help": "How long rclone keeps remote directory listings. Shorter values see server-side changes sooner but browse slower.",
        "buffer_size_help": "Memory read buffer per open file. Larger values can improve sequential reads but use more RAM.",
        "mount_workers_help": "Maximum number of configs mounted in parallel during Mount all. Higher values are faster but can trigger SSH rate limits.",
        "unmount_workers_help": "Maximum number of configs unmounted in parallel during Unmount all. Higher values are usually safe but can make errors arrive together.",
        "startup_all_help": "Creates or removes login-time mount jobs for all saved configs on supported platforms.",
        "startup_all_macos_note": "macOS login mount takes effect on the next login.",
        "startup_config_failed": "Some login mount jobs could not be updated. Details were written to: {path}",
        "dependency_help": "Checks dependencies. rclone is bundled in releases; Windows system dependencies can use winget, but the manual help also shows direct installer links.",
        "updates_help": "Checks the latest SSH MountMate release on GitHub and shows the matching download for this platform.",
        "logs_help": "Open recent rclone mount logs for a saved config. Useful for diagnosing failed mounts.",
        "licenses_help": "Show bundled third-party notices and license text.",
        "updates_title": "SSH MountMate updates",
        "startup_all": "Mount all configs on login",
        "language": "Language",
        "save_settings": "Save settings",
        "settings_saved": "Settings saved.",
        "installing_deps": "Installing missing dependencies...",
        "deps_complete": "Dependency check complete.",
        "deps_failed": "Dependency installation failed.",
        "mount": "Mount",
        "unmount": "Unmount",
        "open_folder": "Open mounted folder",
        "edit_mount": "Edit mount information",
        "edit_mounted_disabled": "Unmount before editing this config",
        "delete_config": "Delete this config",
        "refresh_remote": "Refresh remote directory cache",
        "refresh_unavailable": "Remount this config to enable refresh",
        "view_log": "View mount log",
        "select_log_config": "Config",
        "copy": "Copy",
        "close": "Close",
        "error_details": "Error details",
        "mount_log": "Mount log",
        "copied": "Copied.",
        "remote_refreshed": "Remote directory cache refreshed.",
        "checking_capacity": "checking capacity",
        "unknown_capacity": "unknown capacity",
        "capacity_used": "{used} / {total} used ({percent}%)",
        "mounted_status": "mounted",
        "stopped_status": "stopped",
        "stale_status": "stale",
        "checking_status": "checking",
        "mounted_at": "Mounted {remote} at {mountpoint}",
        "unmounted": "Unmounted.",
        "mount_before_open": "Mount this config before opening its folder.",
        "delete_mounted_confirm": "{name} is mounted. Unmount and delete this config?",
        "delete_confirm": "Delete config {name}?",
        "deleted": "Deleted {name}.",
        "add_config_title": "Add config",
        "edit_config_title": "Edit config",
        "source": "Source",
        "ssh_config": "SSH config",
        "ssh_config_batch": "SSH config (batch)",
        "sai_cluster": "SAI cluster",
        "ssh_config_file": "SSH config file",
        "browse": "Browse",
        "preview": "Preview",
        "import_configs": "Import configs",
        "no_importable_hosts": "No concrete Host entries found in this SSH config.",
        "imported_configs": "Imported {count} configs.",
        "batch_skipped": "Skipped {count} duplicate or invalid configs.",
        "batch_import_notice": "Importing {new_count} new configs. {skip_count} duplicate or invalid configs will be skipped.",
        "batch_conflicts": "Import plan",
        "batch_import": "Import",
        "batch_ignore": "Ignore",
        "batch_overwrite": "Overwrite",
        "batch_overwrite_help": "Overwrite updates the saved SSH connection fields and keeps local mount settings such as remote path and mountpoint.",
        "batch_details": "Details",
        "batch_detail_title": "Batch import details",
        "batch_detail_text": "Host: {host}\nStatus: {status}\nReason: {reason}\nAction: {action}\n\nImported config:\n{server}\n\nMatched saved config:\n{match}",
        "manual": "Manual",
        "ssh_host": "SSH Host",
        "name": "Name",
        "ip_host": "IP / Host",
        "user": "User",
        "port": "Port",
        "auth": "Auth",
        "connection_method": "Connection",
        "rclone_native": "rclone native SFTP",
        "openssh": "OpenSSH",
        "openssh_help": "OpenSSH uses your system ssh command. Add passphrase-protected keys to ssh-agent first; saved key passphrases are not used in this mode.",
        "write_ssh_config": "Write SSH config",
        "copy_key_to_ssh_dir": "Copy key to ~/.ssh",
        "ssh_config_write_help": "Creates an app-managed Host entry under ~/.ssh/ssh-mountmate.d and includes it from ~/.ssh/config. Passwords and key passphrases are never written there.",
        "copy_key_help": "Copies the selected private key into ~/.ssh and writes the copied path to SSH config and this mount profile.",
        "key": "Key",
        "password_auth": "Password",
        "key_file": "Key file",
        "key_passphrase": "Key passphrase",
        "password": "Password",
        "remote_path": "Remote path",
        "mountpoint": "Mountpoint",
        "home_mountpoint": "User folder (~/mnt/name)",
        "mountpoint_help": "Use Auto, a drive letter on Windows, or a custom absolute folder. macOS/Linux folders are created if missing. Windows folder mountpoints need an existing parent and a non-existing target folder.",
        "invalid_mountpoint": "Invalid mountpoint: {reason}",
        "save": "Save",
        "cancel": "Cancel",
        "name_required": "Name is required.",
        "host_user_required": "IP/Host and user are required.",
        "field_required": "{field} is required.",
        "port_invalid": "Port must be a number from 1 to 65535.",
        "ssh_host_required": "SSH Host is required before writing SSH config.",
        "ssh_host_invalid": "SSH Host can only contain letters, numbers, dots, underscores, hyphens, and colons.",
        "key_file_required": "Select a private key file.",
        "key_file_not_found": "Key file not found: {path}",
        "private_key_required": "Select the private key file, not the .pub public key file.",
        "duplicate_target": "A config for the same IP/Host, user, and port already exists: {name}",
        "password_required": "Password is required.",
    },
    "zh": {
        "ready": "就绪",
        "loading_configs": "正在加载配置...",
        "no_configs": "暂无配置。",
        "settings": "设置",
        "add_config": "新增配置",
        "refresh": "刷新",
        "mount_all": "批量挂载",
        "unmount_all": "批量取消挂载",
        "mount_all_started": "正在挂载 {count} 个配置...",
        "unmount_all_started": "正在取消挂载 {count} 个配置...",
        "batch_complete": "批量操作完成，已处理 {done}/{count} 个。",
        "batch_busy": "已有批量操作正在执行。",
        "operation_busy": "这个配置正在处理中。",
        "mount_started": "正在挂载 {name}...",
        "unmount_started": "正在取消挂载 {name}...",
        "checking_deps": "正在检查依赖...",
        "check_dependencies": "检查依赖",
        "install_missing_dependencies": "安装缺失依赖",
        "check_updates": "检查程序更新",
        "checking_updates": "正在检查程序更新...",
        "update_check_failed": "检查更新失败",
        "view_mount_logs": "查看挂载日志",
        "view_licenses": "查看许可证",
        "missing_dependencies": "缺少依赖：{items}。现在安装或显示安装说明吗？",
        "deps_status": "rclone：{rclone}    {mount_dep}：{mount}    ssh：{ssh}",
        "ok": "正常",
        "missing": "缺失",
        "cache_root": "缓存目录",
        "vfs_cache_mode": "VFS 缓存模式",
        "max_cache_size": "最大缓存大小",
        "max_cache_age": "最大缓存寿命",
        "min_free_space": "最小剩余空间",
        "write_back": "写回延迟",
        "dir_cache_time": "目录缓存",
        "buffer_size": "读取缓冲",
        "mount_workers": "挂载并行数",
        "unmount_workers": "取消挂载并行数",
        "language_help": "自动模式会在中文系统使用中文，其他系统使用英文。",
        "cache_root_help": "rclone VFS 本地缓存目录。建议放在速度较快且空间充足的磁盘。",
        "vfs_cache_mode_help": "控制本地文件缓存方式。模式越高，应用兼容性通常越好，但会占用更多磁盘。",
        "max_cache_size_help": "VFS 缓存最大占用空间。默认表示不由 rclone 强制限制。",
        "max_cache_age_help": "缓存对象可保留多久后允许被清理。默认是 1 小时。",
        "min_free_space_help": "为其他应用保留的本地磁盘剩余空间。",
        "write_back_help": "文件变更后延迟多久写回服务器。更长延迟可缓解频繁小写入带来的抖动。",
        "dir_cache_time_help": "rclone 保留远程目录列表的时间。越短越容易看到服务器端变化，但浏览会更频繁访问服务器。",
        "buffer_size_help": "每个打开文件使用的内存读取缓冲。更大可能改善顺序读取，但会占用更多内存。",
        "mount_workers_help": "批量挂载时最多同时处理多少个配置。更大更快，但可能触发 SSH 连接限制。",
        "unmount_workers_help": "批量取消挂载时最多同时处理多少个配置。通常可以比挂载更高，但错误可能集中出现。",
        "startup_all_help": "在支持的平台上为全部已保存配置创建或删除登录挂载任务。",
        "startup_all_macos_note": "macOS 登录挂载会在下次登录时生效。",
        "startup_config_failed": "部分登录挂载任务未能更新，详情已写入：{path}",
        "dependency_help": "检查依赖。Release 内置 rclone；Windows 系统依赖可以使用 winget，手动说明里也会给出直接下载安装入口。",
        "updates_help": "检查 GitHub Releases 上的最新 SSH MountMate，并显示当前平台匹配的下载包。",
        "logs_help": "打开某个已保存配置最近的 rclone 挂载日志，用于排查挂载失败。",
        "licenses_help": "查看内置第三方声明和许可证文本。",
        "updates_title": "SSH MountMate 更新",
        "startup_all": "登录时挂载全部配置",
        "language": "语言",
        "save_settings": "保存设置",
        "settings_saved": "设置已保存。",
        "installing_deps": "正在安装缺失依赖...",
        "deps_complete": "依赖检查完成。",
        "deps_failed": "依赖安装失败。",
        "mount": "挂载",
        "unmount": "取消挂载",
        "open_folder": "打开挂载目录",
        "edit_mount": "编辑挂载信息",
        "edit_mounted_disabled": "请先取消挂载，再编辑此配置",
        "delete_config": "删除此配置",
        "refresh_remote": "刷新远程目录缓存",
        "refresh_unavailable": "重新挂载后才能刷新缓存",
        "view_log": "查看挂载日志",
        "select_log_config": "配置",
        "copy": "复制",
        "close": "关闭",
        "error_details": "错误详情",
        "mount_log": "挂载日志",
        "copied": "已复制。",
        "remote_refreshed": "远程目录缓存已刷新。",
        "checking_capacity": "正在检查容量",
        "unknown_capacity": "容量未知",
        "capacity_used": "已用 {used} / {total}（{percent}%）",
        "mounted_status": "已挂载",
        "stopped_status": "未挂载",
        "stale_status": "状态过期",
        "checking_status": "检查中",
        "mounted_at": "已挂载 {remote} 到 {mountpoint}",
        "unmounted": "已取消挂载。",
        "mount_before_open": "请先挂载此配置，再打开目录。",
        "delete_mounted_confirm": "{name} 正在挂载。是否取消挂载并删除此配置？",
        "delete_confirm": "删除配置 {name}？",
        "deleted": "已删除 {name}。",
        "add_config_title": "新增配置",
        "edit_config_title": "编辑配置",
        "source": "来源",
        "ssh_config": "SSH 配置",
        "ssh_config_batch": "SSH 配置（批量）",
        "sai_cluster": "SAI 集群",
        "ssh_config_file": "SSH 配置文件",
        "browse": "浏览",
        "preview": "预览",
        "import_configs": "导入配置",
        "no_importable_hosts": "这个 SSH config 中没有找到具体 Host。",
        "imported_configs": "已导入 {count} 个配置。",
        "batch_skipped": "已跳过 {count} 个重复或无效配置。",
        "batch_import_notice": "将导入 {new_count} 个新配置，并跳过 {skip_count} 个重复或无效配置。",
        "batch_conflicts": "导入计划",
        "batch_import": "导入",
        "batch_ignore": "忽略",
        "batch_overwrite": "覆盖",
        "batch_overwrite_help": "覆盖只更新已保存配置里的 SSH 连接字段，并保留远程路径、挂载点等本地挂载设置。",
        "batch_details": "详情",
        "batch_detail_title": "批量导入详情",
        "batch_detail_text": "Host：{host}\n状态：{status}\n原因：{reason}\n动作：{action}\n\n拟导入配置：\n{server}\n\n匹配到的已保存配置：\n{match}",
        "manual": "手动",
        "ssh_host": "SSH Host",
        "name": "名称",
        "ip_host": "IP / 主机",
        "user": "用户名",
        "port": "端口",
        "auth": "认证",
        "connection_method": "连接方式",
        "rclone_native": "rclone 原生 SFTP",
        "openssh": "OpenSSH",
        "openssh_help": "OpenSSH 会使用系统 ssh 命令。带短语的密钥请先加入 ssh-agent；此模式不会使用已保存的密钥短语。",
        "write_ssh_config": "写入 SSH config",
        "copy_key_to_ssh_dir": "复制密钥到 ~/.ssh",
        "ssh_config_write_help": "在 ~/.ssh/ssh-mountmate.d 下创建由应用管理的 Host，并从 ~/.ssh/config Include。密码和密钥短语不会写入 SSH config。",
        "copy_key_help": "把选中的私钥复制到 ~/.ssh，并把复制后的路径写入 SSH config 和当前挂载配置。",
        "key": "密钥",
        "password_auth": "密码",
        "key_file": "密钥文件",
        "key_passphrase": "密钥短语",
        "password": "密码",
        "remote_path": "远程路径",
        "mountpoint": "挂载点",
        "home_mountpoint": "用户文件夹 (~/mnt/名称)",
        "mountpoint_help": "可以使用 Auto、Windows 盘符，或自定义绝对路径文件夹。macOS/Linux 文件夹不存在时会创建；Windows 文件夹挂载要求父目录已存在，目标文件夹本身不能已存在。",
        "invalid_mountpoint": "挂载点无效：{reason}",
        "save": "保存",
        "cancel": "取消",
        "name_required": "名称必填。",
        "host_user_required": "IP/主机和用户名必填。",
        "field_required": "{field}必填。",
        "port_invalid": "端口必须是 1 到 65535 之间的数字。",
        "ssh_host_required": "写入 SSH config 前必须填写 SSH Host。",
        "ssh_host_invalid": "SSH Host 只能包含字母、数字、点、下划线、短横线和冒号。",
        "key_file_required": "请选择私钥文件。",
        "key_file_not_found": "找不到密钥文件：{path}",
        "private_key_required": "请选择私钥文件，不要选择 .pub 公钥文件。",
        "duplicate_target": "已存在相同 IP/主机、用户名和端口的配置：{name}",
        "password_required": "密码必填。",
    },
}
def app_dir() -> Path:
    return rsshmount.app_config_dir()


def servers_path() -> Path:
    return app_dir() / "servers.json"


def settings_path() -> Path:
    return app_dir() / "settings.json"


def default_settings() -> dict:
    return {
        "cache_root": str(rsshmount.xdg_cache_home()),
        "vfs_cache_mode": "writes",
        "vfs_cache_max_size": "",
        "vfs_cache_max_age": "",
        "vfs_cache_min_free_space": "",
        "vfs_write_back": "",
        "dir_cache_time": "",
        "buffer_size": "",
        "mount_all_workers": DEFAULT_MOUNT_ALL_WORKERS,
        "unmount_all_workers": DEFAULT_UNMOUNT_ALL_WORKERS,
        "startup_all": False,
        "language": "auto",
    }


def load_settings() -> dict:
    settings = default_settings()
    path = settings_path()
    if path.exists():
        try:
            settings.update(json.loads(path.read_text(encoding="utf-8")))
        except Exception:
            pass
    return settings


def save_settings(settings: dict) -> None:
    app_dir().mkdir(parents=True, exist_ok=True)
    settings_path().write_text(json.dumps(settings, indent=2), encoding="utf-8")


def configured_cache_dir(host: str) -> Path:
    settings = load_settings()
    root = settings.get("cache_root") or default_settings()["cache_root"]
    return Path(root).expanduser() / host


def setting_to_choice(value: str, default_choice: str) -> str:
    return value if value else default_choice


def choice_to_setting(value: str) -> str:
    return "" if value.startswith("default ") else value


def bounded_int(value, default: int, minimum: int = 1, maximum: int = 12) -> int:
    try:
        number = int(value)
    except (TypeError, ValueError):
        return default
    return max(minimum, min(maximum, number))


def setting_worker_choice(value, default: int) -> str:
    number = bounded_int(value, default)
    text = str(number)
    return text if text in BATCH_WORKER_CHOICES else str(default)


def system_language() -> str:
    env_lang = " ".join(
        str(os.environ.get(key) or "")
        for key in ("SSH_MOUNTMATE_LANG", "LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG")
    ).lower()
    if "zh" in env_lang or "chinese" in env_lang:
        return "zh"
    if os.name == "nt":
        try:
            buffer = ctypes.create_unicode_buffer(85)
            if ctypes.windll.kernel32.GetUserDefaultLocaleName(buffer, len(buffer)):
                if buffer.value.lower().startswith("zh"):
                    return "zh"
        except Exception:
            pass
        try:
            lang_id = int(ctypes.windll.kernel32.GetUserDefaultUILanguage())
            primary_lang = lang_id & 0x3FF
            if primary_lang == 0x04:
                return "zh"
        except Exception:
            pass
    try:
        lang = locale.getlocale()[0] or locale.getdefaultlocale()[0] or locale.getlocale(locale.LC_CTYPE)[0] or ""
    except Exception:
        lang = ""
    return "zh" if lang.lower().startswith("zh") else "en"


def effective_language(settings: dict | None = None) -> str:
    value = (settings or load_settings()).get("language", "auto")
    if value == "zh":
        return "zh"
    if value == "en":
        return "en"
    return system_language()


def tr_lang(lang: str, key: str, **kwargs) -> str:
    text = TEXT.get(lang, TEXT["en"]).get(key, TEXT["en"].get(key, key))
    return text.format(**kwargs) if kwargs else text


def language_choice_from_setting(value: str) -> str:
    return LANGUAGE_CHOICES.get(value or "auto", LANGUAGE_CHOICES["auto"])


def language_setting_from_choice(value: str) -> str:
    for key, label in LANGUAGE_CHOICES.items():
        if value == label:
            return key
    return "auto"


def bundled_dir() -> Path:
    if getattr(sys, "frozen", False):
        return Path(getattr(sys, "_MEIPASS", Path(sys.executable).resolve().parent))
    return Path(__file__).resolve().parent


def asset_dir() -> Path:
    return bundled_dir() / "assets"


def embedded_chinese_font() -> Path:
    return asset_dir() / "fonts" / "NotoSansCJKsc-Regular.otf"


def load_embedded_chinese_font() -> bool:
    font_path = embedded_chinese_font()
    if not font_path.exists() or os.name != "nt":
        return font_path.exists()
    try:
        return bool(ctypes.windll.gdi32.AddFontResourceExW(str(font_path), 0x10, 0))
    except Exception:
        return False


def configure_default_fonts(root: Tk, lang: str) -> None:
    family = FONT_FAMILY_ZH if lang == "zh" and load_embedded_chinese_font() else FONT_FAMILY_EN
    for name in ["TkDefaultFont", "TkTextFont", "TkMenuFont", "TkHeadingFont", "TkCaptionFont", "TkSmallCaptionFont"]:
        try:
            tkfont.nametofont(name).configure(family=family)
        except Exception:
            pass


def bundled_rclone_path() -> Path:
    return bundled_dir() / "bin" / ("rclone.exe" if os.name == "nt" else "rclone")


def refresh_windows_path_env() -> None:
    if os.name != "nt":
        return
    try:
        import winreg
    except ImportError:
        return

    registry_paths: list[str] = []
    keys = [
        (winreg.HKEY_CURRENT_USER, r"Environment"),
        (winreg.HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment"),
    ]
    for root, subkey in keys:
        try:
            with winreg.OpenKey(root, subkey) as key:
                value, value_type = winreg.QueryValueEx(key, "Path")
        except OSError:
            continue
        if value_type == winreg.REG_EXPAND_SZ:
            value = winreg.ExpandEnvironmentStrings(value)
        registry_paths.extend(part for part in str(value).split(os.pathsep) if part)

    current_paths = [part for part in os.environ.get("PATH", "").split(os.pathsep) if part]
    merged: list[str] = []
    seen: set[str] = set()
    for part in current_paths + registry_paths:
        key = part.casefold()
        if key not in seen:
            seen.add(key)
            merged.append(part)
    if merged:
        os.environ["PATH"] = os.pathsep.join(merged)


def known_rclone_paths() -> list[Path]:
    candidates: list[Path] = [managed_rclone_path()]
    if os.name != "nt":
        home = Path.home()
        candidates.extend(
            [
                home / ".local" / "bin" / "rclone",
                Path("/opt/homebrew/bin/rclone"),
                Path("/usr/local/bin/rclone"),
                Path("/opt/local/bin/rclone"),
                Path("/usr/bin/rclone"),
                Path("/snap/bin/rclone"),
            ]
        )
        return candidates

    localappdata = os.environ.get("LOCALAPPDATA")
    programfiles = os.environ.get("ProgramFiles")
    programfiles_x86 = os.environ.get("ProgramFiles(x86)")

    if localappdata:
        local = Path(localappdata)
        candidates.append(local / "Microsoft" / "WinGet" / "Links" / "rclone.exe")
        packages = local / "Microsoft" / "WinGet" / "Packages"
        if packages.exists():
            try:
                candidates.extend(packages.glob("Rclone.Rclone_*/**/rclone.exe"))
            except OSError:
                pass
    for root in [programfiles, programfiles_x86]:
        if root:
            candidates.append(Path(root) / "rclone" / "rclone.exe")
    return candidates


def resolve_rclone_path() -> str:
    bundled = bundled_rclone_path()
    if bundled.exists():
        return str(bundled)
    if os.name != "nt":
        augment_process_path()
    found = shutil.which("rclone.exe" if os.name == "nt" else "rclone") or shutil.which("rclone")
    if found:
        return found
    for candidate in known_rclone_paths():
        if candidate.exists():
            return str(candidate)
    return ""


def create_no_window() -> int:
    return getattr(subprocess, "CREATE_NO_WINDOW", 0) if os.name == "nt" else 0


def run(cmd: list[str], *, check=True, capture=False) -> subprocess.CompletedProcess:
    return subprocess.run(
        cmd,
        check=check,
        text=True,
        stdout=subprocess.PIPE if capture else None,
        stderr=subprocess.PIPE if capture else None,
        creationflags=create_no_window(),
    )


def free_local_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def load_servers() -> list[dict]:
    path = servers_path()
    if not path.exists():
        return []
    try:
        servers = json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        return []
    if not isinstance(servers, list):
        return []
    servers, id_changed = normalize_server_ids(servers)
    servers, name_changed = normalize_server_names(servers)
    changed = id_changed or name_changed
    if changed:
        save_servers(servers)
    return servers


def list_ssh_config_hosts(config_path: str | Path | None = None, seen: set[Path] | None = None) -> list[str]:
    entries = list_ssh_config_host_entries(config_path, seen)
    unique: list[str] = []
    for entry in entries:
        host = entry["host"]
        if host not in unique:
            unique.append(host)
    return unique


def list_ssh_config_host_entries(config_path: str | Path | None = None, seen: set[Path] | None = None) -> list[dict]:
    config = Path(config_path).expanduser() if config_path else (Path.home() / ".ssh" / "config")
    seen = seen or set()
    try:
        resolved = config.resolve()
    except OSError:
        resolved = config
    if resolved in seen or not config.exists():
        return []
    seen.add(resolved)

    entries: list[dict] = []
    for line_no, raw_line in enumerate(config.read_text(encoding="utf-8", errors="ignore").splitlines(), 1):
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        words = line.split()
        if not words:
            continue
        keyword = words[0].lower()
        if keyword == "include":
            for pattern in words[1:]:
                expanded = os.path.expanduser(pattern)
                if not os.path.isabs(expanded):
                    expanded = str(config.parent / expanded)
                for included in glob.glob(expanded):
                    entries.extend(list_ssh_config_host_entries(Path(included), seen))
        elif keyword == "host":
            for host in words[1:]:
                if "*" not in host and "?" not in host and "!" not in host:
                    entries.append({"host": host, "path": config, "line": line_no, "raw": raw_line})
    return entries


def user_ssh_dir() -> Path:
    return Path.home() / ".ssh"


def managed_ssh_config_dir() -> Path:
    return user_ssh_dir() / "ssh-mountmate.d"


def ssh_config_include_line() -> str:
    return "Include ~/.ssh/ssh-mountmate.d/*.conf"


def ensure_user_ssh_dir() -> Path:
    ssh_dir = user_ssh_dir()
    ssh_dir.mkdir(parents=True, exist_ok=True)
    if os.name != "nt":
        try:
            ssh_dir.chmod(0o700)
        except OSError:
            pass
    return ssh_dir


def ensure_managed_ssh_include() -> None:
    ssh_dir = ensure_user_ssh_dir()
    managed_ssh_config_dir().mkdir(parents=True, exist_ok=True)
    if os.name != "nt":
        try:
            managed_ssh_config_dir().chmod(0o700)
        except OSError:
            pass
    config = ssh_dir / "config"
    include = ssh_config_include_line()
    if config.exists():
        text = config.read_text(encoding="utf-8", errors="ignore")
        for line in text.splitlines():
            stripped = line.strip()
            if stripped.lower() == include.lower():
                return
        prefix = "" if text.endswith(("\n", "\r")) or not text else "\n"
        config.write_text(f"{text}{prefix}{include}\n", encoding="utf-8")
    else:
        config.write_text(f"{include}\n", encoding="utf-8")
    if os.name != "nt":
        try:
            config.chmod(0o600)
        except OSError:
            pass


def ssh_safe_name(value: str) -> str:
    cleaned = "".join(ch if ch.isalnum() or ch in "._-" else "_" for ch in (value or "").strip())
    return cleaned.strip("._-") or f"host-{uuid.uuid4().hex[:8]}"


def managed_ssh_config_file(host_alias: str) -> Path:
    return managed_ssh_config_dir() / f"{ssh_safe_name(host_alias)}.conf"


def shell_home_path(path: Path) -> str:
    try:
        relative = path.expanduser().resolve().relative_to(Path.home().resolve())
        return "~/" + relative.as_posix()
    except Exception:
        return path.expanduser().as_posix()


def ssh_config_quote(value: str) -> str:
    text = str(value)
    if not text:
        return '""'
    if any(ch.isspace() for ch in text) or '"' in text or "\\" in text:
        return '"' + text.replace("\\", "\\\\").replace('"', '\\"') + '"'
    return text


def normalized_port(value: object) -> str:
    text = str(value or "").strip()
    if not text:
        raise ValueError("Port is required.")
    if not text.isdigit():
        raise ValueError("Port must be a number from 1 to 65535.")
    port = int(text)
    if port < 1 or port > 65535:
        raise ValueError("Port must be a number from 1 to 65535.")
    return str(port)


def validate_ssh_host_alias(host_alias: str) -> str:
    value = str(host_alias or "").strip()
    if not value:
        raise ValueError("SSH Host is required to write SSH config.")
    if value != str(host_alias or ""):
        raise ValueError("SSH Host must not start or end with whitespace.")
    if any(ch.isspace() or ord(ch) < 32 for ch in value):
        raise ValueError("SSH Host must not contain whitespace or control characters.")
    if any(ch in value for ch in "*?!#\"'\\"):
        raise ValueError("SSH Host must not contain wildcards, comments, quotes, or backslashes.")
    if value.startswith("-"):
        raise ValueError("SSH Host must not start with '-'.")
    if not re.fullmatch(r"[A-Za-z0-9_.:-]+", value):
        raise ValueError("SSH Host can only contain letters, numbers, dots, underscores, hyphens, and colons.")
    return value


def validate_ssh_config_scalar(value: object, field_name: str) -> str:
    text = str(value or "").strip()
    if not text:
        raise ValueError(f"{field_name} is required to write SSH config.")
    if any(ch.isspace() or ord(ch) < 32 for ch in text):
        raise ValueError(f"{field_name} must not contain whitespace or control characters.")
    if text.startswith("-"):
        raise ValueError(f"{field_name} must not start with '-'.")
    return text


def validate_private_key_path(path_value: str) -> str:
    text = str(path_value or "").strip()
    if not text:
        raise ValueError("Select a private key file.")
    path = Path(text).expanduser()
    if not path.exists() or not path.is_file():
        raise ValueError(f"Key file not found: {text}")
    if path.suffix == ".pub":
        raise ValueError("Select the private key file, not the .pub public key file.")
    return text


def validate_managed_ssh_config_server(server: dict) -> dict:
    host_alias = validate_ssh_host_alias(server.get("host_alias") or server.get("name") or server.get("id") or "")
    host = validate_ssh_config_scalar(server.get("host"), "IP/Host")
    user = validate_ssh_config_scalar(server.get("user"), "User")
    port = normalized_port(server.get("port"))
    key_file = str(server.get("key_file") or "").strip()
    if key_file:
        validate_private_key_path(key_file)
    return {"host_alias": host_alias, "host": host, "user": user, "port": port, "key_file": key_file}


def copy_key_to_user_ssh(source: str, host_alias: str) -> str:
    source = validate_private_key_path(source)
    source_path = Path(source).expanduser()
    ssh_dir = ensure_user_ssh_dir()
    base = ssh_safe_name(host_alias)
    suffix = source_path.suffix
    target = ssh_dir / f"{base}{suffix}"
    if source_path.resolve() == target.resolve():
        return str(target)
    if target.exists():
        for index in range(2, 1000):
            candidate = ssh_dir / f"{base}-{index}{suffix}"
            if not candidate.exists():
                target = candidate
                break
        else:
            target = ssh_dir / f"{base}-{uuid.uuid4().hex[:8]}{suffix}"
    shutil.copy2(source_path, target)
    if os.name != "nt":
        try:
            target.chmod(0o600)
        except OSError:
            pass
    return str(target)


def write_managed_ssh_config(server: dict) -> Path:
    validated = validate_managed_ssh_config_server(server)
    host_alias = validated["host_alias"]
    host = validated["host"]
    user = validated["user"]
    port = validated["port"]
    ensure_managed_ssh_include()
    target = managed_ssh_config_file(host_alias)
    lines = [
        "# Managed by SSH MountMate. Edit from the app when possible.",
        f"Host {ssh_config_quote(host_alias)}",
        f"    HostName {ssh_config_quote(host)}",
        f"    User {ssh_config_quote(user)}",
        f"    Port {ssh_config_quote(port)}",
    ]
    key_file = validated["key_file"]
    if key_file:
        lines.append(f"    IdentityFile {ssh_config_quote(shell_home_path(Path(key_file)))}")
        lines.append("    IdentitiesOnly yes")
    target.write_text("\n".join(lines) + "\n", encoding="utf-8")
    if os.name != "nt":
        try:
            target.chmod(0o600)
        except OSError:
            pass
    return target


def remove_managed_ssh_config(server: dict) -> None:
    path_value = str(server.get("managed_ssh_config_path") or "")
    candidates: list[Path] = []
    if path_value:
        candidates.append(Path(path_value).expanduser())
    if server.get("host_alias"):
        candidates.append(managed_ssh_config_file(str(server["host_alias"])))
    managed_root = managed_ssh_config_dir().resolve()
    for candidate in candidates:
        try:
            resolved = candidate.resolve()
            if resolved.parent == managed_root and resolved.exists():
                resolved.unlink()
        except OSError:
            pass


def save_servers(servers: list[dict]) -> None:
    app_dir().mkdir(parents=True, exist_ok=True)
    servers_path().write_text(json.dumps(servers, indent=2), encoding="utf-8")


def sanitize_server_id(value: str) -> str:
    cleaned = "".join(ch if ch.isalnum() or ch in "._-" else "_" for ch in (value or "").strip())
    return cleaned.strip("._-") or f"server-{uuid.uuid4().hex[:8]}"


def make_unique_server_id(base: str, used: set[str]) -> str:
    root = sanitize_server_id(base)
    if root not in used:
        return root
    for number in range(2, 1000):
        candidate = f"{root}-{number}"
        if candidate not in used:
            return candidate
    while True:
        candidate = f"{root}-{uuid.uuid4().hex[:8]}"
        if candidate not in used:
            return candidate


def server_name_base(server: dict) -> str:
    return str(server.get("name") or server.get("host_alias") or server.get("host") or server.get("id") or "Server").strip() or "Server"


def server_name_key(name: str) -> str:
    return str(name or "").strip().casefold()


def mountpoint_folder_name_for_name(name: str) -> str:
    return sanitize_server_id(name)


def mountpoint_folder_key_for_name(name: str) -> str:
    return mountpoint_folder_name_for_name(name).casefold()


def make_unique_server_name(base: str, used_names: set[str], used_mount_folders: set[str]) -> str:
    root = str(base or "").strip() or "Server"

    def available(candidate: str) -> bool:
        return server_name_key(candidate) not in used_names and mountpoint_folder_key_for_name(candidate) not in used_mount_folders

    if available(root):
        return root
    for number in range(2, 1000):
        candidate = f"{root} {number}"
        if available(candidate):
            return candidate
    while True:
        candidate = f"{root} {uuid.uuid4().hex[:8]}"
        if available(candidate):
            return candidate


def add_used_server_name(name: str, used_names: set[str], used_mount_folders: set[str]) -> None:
    used_names.add(server_name_key(name))
    used_mount_folders.add(mountpoint_folder_key_for_name(name))


def mountpoint_folder_name(server: dict) -> str:
    return mountpoint_folder_name_for_name(server_name_base(server))


def server_remote_name_for_state(server: dict) -> str:
    return server.get("host_alias", "") if server.get("mode") == "ssh_config" else server.get("id", "")


def expected_remote_for_state(server: dict) -> str:
    return rsshmount.remote_spec(server_remote_name_for_state(server), server.get("remote_path") or "")


def state_matches_server(state: dict, server: dict) -> bool:
    return bool(state.get("remote")) and state.get("remote") == expected_remote_for_state(server)


def normalize_server_ids(servers: list[dict]) -> tuple[list[dict], bool]:
    normalized = [dict(server) for server in servers]
    groups: dict[str, list[int]] = {}
    for index, server in enumerate(normalized):
        groups.setdefault(server.get("id") or "", []).append(index)

    used: set[str] = set()
    changed = False
    for original_id, indexes in groups.items():
        keep_index = indexes[0]
        if original_id and len(indexes) > 1:
            try:
                state = json.loads((rsshmount.app_state_dir() / f"{original_id}.json").read_text(encoding="utf-8"))
            except Exception:
                state = {}
            for index in indexes:
                if state_matches_server(state, normalized[index]):
                    keep_index = index
                    break
        for index in indexes:
            server = normalized[index]
            current_id = server.get("id") or ""
            if current_id and index == keep_index and current_id not in used:
                used.add(current_id)
                continue
            base = server.get("name") or server.get("host_alias") or server.get("host") or current_id
            server["id"] = make_unique_server_id(base, used)
            used.add(server["id"])
            changed = True
    return normalized, changed


def normalize_server_names(servers: list[dict]) -> tuple[list[dict], bool]:
    normalized = [dict(server) for server in servers]
    used_names: set[str] = set()
    used_mount_folders: set[str] = set()
    changed = False
    for server in normalized:
        original = str(server.get("name") or "").strip()
        unique = make_unique_server_name(server_name_base(server), used_names, used_mount_folders)
        if original != unique:
            server["name"] = unique
            changed = True
        add_used_server_name(unique, used_names, used_mount_folders)
    return normalized, changed


def server_source_value(server: dict) -> str:
    if server.get("source"):
        return str(server.get("source"))
    return "ssh_config" if server.get("mode") == "ssh_config" else "manual"


def same_password_target(existing: dict, result: dict) -> bool:
    if server_source_value(existing) != server_source_value(result):
        return False
    return all(
        str(existing.get(key) or "") == str(result.get(key) or "")
        for key in ("host_alias", "host", "user", "port", "auth", "connection_method")
    )


def same_key_passphrase_target(existing: dict, result: dict) -> bool:
    return (
        str(existing.get("auth") or "") == str(result.get("auth") or "")
        and str(existing.get("key_file") or "") == str(result.get("key_file") or "")
        and connection_method_value(existing) == connection_method_value(result)
    )


def connection_method_value(server: dict) -> str:
    return str(server.get("connection_method") or "native")


def server_label(server: dict) -> str:
    name = server.get("name") or server.get("id")
    mode = server.get("source") or server.get("mode", "")
    mountpoint = current_mountpoint(server) if server.get("mountpoint") == HOME_MOUNTPOINT_VALUE else server.get("mountpoint") or "Auto"
    status = mount_status(server)
    return f"{name}  [{mode}]  {mountpoint}  - {status}"


def server_state_file(server: dict) -> Path:
    return rsshmount.app_state_dir() / f"{server['id']}.json"


def running_pid_set() -> set[int]:
    if os.name != "nt":
        return set()
    result = subprocess.run(
        ["tasklist", "/FI", "IMAGENAME eq rclone.exe", "/FO", "CSV", "/NH"],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        creationflags=create_no_window(),
    )
    pids: set[int] = set()
    for line in result.stdout.splitlines():
        parts = [part.strip().strip('"') for part in line.split(",")]
        if len(parts) > 1 and parts[1].isdigit():
            pids.add(int(parts[1]))
    return pids


def running_rclone_processes() -> dict[int, str]:
    if os.name != "nt":
        return {}
    command = (
        "Get-CimInstance Win32_Process -Filter \"Name='rclone.exe'\" | "
        "Select-Object ProcessId,CommandLine | ConvertTo-Json -Compress"
    )
    result = subprocess.run(
        ["powershell.exe", "-NoProfile", "-Command", command],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        creationflags=create_no_window(),
    )
    try:
        data = json.loads(result.stdout.strip() or "[]")
    except json.JSONDecodeError:
        return {}
    if isinstance(data, dict):
        data = [data]
    processes: dict[int, str] = {}
    for item in data:
        try:
            pid = int(item.get("ProcessId", 0))
        except (TypeError, ValueError):
            continue
        if pid:
            processes[pid] = str(item.get("CommandLine") or "")
    return processes


def pid_is_running(pid: int, pid_set: set[int] | None = None) -> bool:
    if pid_set is not None:
        return pid in pid_set
    if os.name == "nt":
        result = subprocess.run(
            ["tasklist", "/FI", f"PID eq {pid}", "/FO", "CSV", "/NH"],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            creationflags=create_no_window(),
        )
        return str(pid) in result.stdout
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False


def mount_status(server: dict) -> str:
    state_file = server_state_file(server)
    if not state_file.exists():
        return "stopped"
    try:
        state = json.loads(state_file.read_text(encoding="utf-8"))
        pid = int(state.get("pid", 0))
    except Exception:
        return "stale"
    return "mounted" if pid and pid_is_running(pid) else "stale"


def mount_status_with_pids(server: dict, pid_set: set[int]) -> str:
    state_file = server_state_file(server)
    if not state_file.exists():
        return "stopped"
    try:
        state = json.loads(state_file.read_text(encoding="utf-8"))
        pid = int(state.get("pid", 0))
    except Exception:
        return "stale"
    return "mounted" if pid and pid_is_running(pid, pid_set) else "stale"


def command_matches_state(command: str, state: dict) -> bool:
    command = command.casefold()
    expected = [state.get("remote", ""), state.get("mountpoint", ""), state.get("log", "")]
    return all(str(value).casefold() in command for value in expected if value)


def process_command(pid: int) -> str:
    if os.name != "nt" or not pid:
        return ""
    return running_rclone_processes().get(pid, "")


def mount_status_with_processes(server: dict, processes: dict[int, str]) -> str:
    state_file = server_state_file(server)
    if not state_file.exists():
        return "stopped"
    try:
        state = json.loads(state_file.read_text(encoding="utf-8"))
        pid = int(state.get("pid", 0))
    except Exception:
        return "stale"
    command = processes.get(pid)
    if not pid or not command:
        return "stale"
    return "mounted" if command_matches_state(command, state) else "stale"


def verified_mount_status(server: dict) -> str:
    if os.name == "nt":
        return mount_status_with_processes(server, running_rclone_processes())
    return mount_status(server)


def mountpoint_ready(mountpoint: str) -> bool:
    try:
        if os.name == "nt" and len(mountpoint) in (2, 3) and mountpoint[1] == ":":
            return rsshmount.windows_drive_in_use(mountpoint)
        if os.name == "nt":
            return Path(mountpoint).exists()
        return Path(mountpoint).is_mount()
    except OSError:
        return False


def resolve_mountpoint(server: dict) -> str:
    configured_mountpoint = server.get("mountpoint") or ""
    if configured_mountpoint == HOME_MOUNTPOINT_VALUE:
        return str(resolve_home_mountpoint(server))
    if not configured_mountpoint or configured_mountpoint.lower() == "auto":
        if os.name != "nt":
            return str(rsshmount.home_mountpoint(mountpoint_folder_name(server)))
        return str(rsshmount.default_mountpoint(remote_name(server)))
    if is_windows_mount_drive(configured_mountpoint):
        return configured_mountpoint
    return str(Path(configured_mountpoint).expanduser())


def resolve_home_mountpoint(server: dict) -> Path:
    base = rsshmount.home_mountpoint(mountpoint_folder_name(server))
    parent = base.parent
    if current_state(server).get("mountpoint"):
        return Path(current_state(server)["mountpoint"]).expanduser()
    for index in range(1, 1000):
        candidate = base if index == 1 else base.with_name(f"{base.name}-{index}")
        try:
            if candidate.is_mount():
                continue
            if os.name == "nt":
                if not candidate.exists():
                    return candidate
                continue
            if not candidate.exists() or candidate.is_dir():
                return candidate
        except OSError:
            continue
    return parent / f"{base.name}-{uuid.uuid4().hex[:8]}"


def is_under_home_mnt(path: Path) -> bool:
    try:
        path.expanduser().resolve(strict=False).relative_to((Path.home() / "mnt").resolve(strict=False))
        return True
    except (OSError, ValueError):
        return False


def prepare_gui_mountpoint(mountpoint: str, *, home_mountpoint: bool = False) -> None:
    path = Path(mountpoint).expanduser()
    value = str(path)
    if home_mountpoint or is_under_home_mnt(path):
        path.parent.mkdir(parents=True, exist_ok=True)
    error = validate_mountpoint_for_mount(value)
    if error:
        raise RuntimeError(f"Invalid mountpoint: {error}")
    if os.name == "nt":
        if value == "*" or rsshmount.is_windows_drive(value):
            return
        return
    path.mkdir(parents=True, exist_ok=True)


def wait_for_mount_ready(
    proc: subprocess.Popen,
    mountpoint: str,
    log_path: Path,
    expected_state: dict,
    *,
    ready_before_start: bool,
    timeout: float = 20.0,
) -> None:
    deadline = time.time() + timeout
    ready_since = 0.0
    while time.time() < deadline:
        if proc.poll() is not None:
            break
        ready_now = mountpoint_ready(mountpoint)
        if ready_before_start:
            if not ready_now:
                ready_before_start = False
            time.sleep(0.25)
            continue
        if ready_now:
            if not ready_since:
                ready_since = time.time()
            if time.time() - ready_since >= 0.75:
                if os.name == "nt":
                    command = process_command(proc.pid)
                    if command and not command_matches_state(command, expected_state):
                        break
                return
        else:
            ready_since = 0.0
        time.sleep(0.25)
    if proc.poll() is None and ready_before_start:
        try:
            log_path.write_text(
                log_path.read_text(encoding="utf-8", errors="ignore")
                + f"\nMountpoint {mountpoint} already existed before this mount attempt.\n",
                encoding="utf-8",
            )
        except OSError:
            pass
    if proc.poll() is None:
        proc.terminate()
        time.sleep(0.5)
        if proc.poll() is None:
            proc.kill()
    tail = ""
    try:
        tail = "\n".join(log_path.read_text(encoding="utf-8", errors="ignore").splitlines()[-12:])
    except OSError:
        pass
    raise RuntimeError(f"Mount did not become ready. See log: {log_path}\n{tail}")


def current_mountpoint(server: dict) -> str:
    state_file = server_state_file(server)
    if state_file.exists():
        try:
            state = json.loads(state_file.read_text(encoding="utf-8"))
            if state.get("mountpoint"):
                return state["mountpoint"]
        except Exception:
            pass
    configured = server.get("mountpoint") or ""
    if configured == HOME_MOUNTPOINT_VALUE:
        return str(rsshmount.home_mountpoint(mountpoint_folder_name(server)))
    if configured and configured.lower() != "auto":
        return configured
    if os.name != "nt":
        return str(rsshmount.home_mountpoint(mountpoint_folder_name(server)))
    return str(rsshmount.default_mountpoint(remote_name(server)))


def current_state(server: dict) -> dict:
    state_file = server_state_file(server)
    if not state_file.exists():
        return {}
    try:
        return json.loads(state_file.read_text(encoding="utf-8"))
    except Exception:
        return {}


def current_log_path(server: dict) -> Path:
    state = current_state(server)
    if state.get("log"):
        return Path(state["log"])
    return rsshmount.app_state_dir() / f"{remote_name(server)}.log"


def display_mountpoint(server: dict) -> str:
    mountpoint = current_mountpoint(server)
    return mountpoint if mountpoint else "Auto"


def display_mountpoint_for_status(server: dict, status: str) -> str:
    configured = server.get("mountpoint") or ""
    if status == "mounted":
        return display_mountpoint(server)
    if configured == HOME_MOUNTPOINT_VALUE:
        return str(rsshmount.home_mountpoint(mountpoint_folder_name(server)))
    if configured and configured.lower() != "auto":
        return configured
    return str(rsshmount.home_mountpoint(mountpoint_folder_name(server))) if os.name != "nt" else "Auto"


def format_capacity_bytes(size: int) -> str:
    units = [("TB", 1024**4), ("GB", 1024**3), ("MB", 1024**2), ("KB", 1024)]
    for unit, factor in units:
        if abs(size) >= factor:
            return f"{size / factor:.1f} {unit}"
    return f"{size} B"


def parse_lustre_project_line(output: str) -> int | None:
    for line in output.splitlines():
        match = re.match(r"\s*(\d+)\s+\S+\s+.+", line)
        if match:
            return int(match.group(1))
    return None


def parse_lustre_quota_kbytes(output: str) -> dict:
    for line in output.splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("Disk ") or stripped.lower().startswith("filesystem"):
            continue
        parts = stripped.split()
        if len(parts) < 4:
            continue
        try:
            used_kb = int(parts[1])
            quota_kb = int(parts[2])
            limit_kb = int(parts[3])
        except ValueError:
            continue
        _ = quota_kb
        total_kb = limit_kb
        if not total_kb:
            return {}
        used = max(used_kb, 0) * 1024
        total = total_kb * 1024
        percent = int(round((used / total) * 100)) if total else 0
        return {"used": used, "total": total, "percent": max(0, min(percent, 100)), "source": "lustre_project_quota"}
    return {}


def remote_path_for_capacity(server: dict) -> str:
    remote_path = str(server.get("remote_path") or "").strip()
    return remote_path or "."


def lustre_project_capacity_info(server: dict) -> dict:
    if server.get("auth") == "password" and not (server.get("source") == "ssh_config" or server.get("ssh_config_managed")):
        return {}
    script = r'''
set -eu
target=${1:-.}
if [ -z "$target" ]; then target=.; fi
if ! command -v lfs >/dev/null 2>&1; then exit 0; fi
if [ -d "$target" ]; then
  resolved=$(cd "$target" 2>/dev/null && pwd -P) || exit 0
else
  resolved=$(readlink -f -- "$target" 2>/dev/null || printf '%s' "$target")
fi
df_out=$(df -P -T "$resolved" 2>/dev/null | awk 'NR==2 {print $2 "\t" $7}')
fstype=${df_out%%	*}
mountpoint=${df_out#*	}
if [ "$fstype" != "lustre" ] || [ -z "$mountpoint" ]; then exit 0; fi
project_out=$(lfs project -d "$resolved" 2>/dev/null || true)
project_id=$(printf '%s\n' "$project_out" | awk 'NF >= 3 && $1 ~ /^[0-9]+$/ {print $1; exit}')
if [ -z "$project_id" ]; then exit 0; fi
quota_out=$(lfs quota -p "$project_id" "$resolved" 2>/dev/null || true)
if ! printf '%s\n' "$quota_out" | awk 'NF >= 4 && $2 ~ /^[0-9]+$/ {found=1} END {exit !found}'; then
  quota_out=$(lfs quota -p "$project_id" "$mountpoint" 2>/dev/null || true)
fi
printf 'SSH_MOUNTMATE_LUSTRE_PROJECT=%s\n' "$project_id"
printf 'SSH_MOUNTMATE_LUSTRE_MOUNT=%s\n' "$mountpoint"
printf '%s\n' "$quota_out"
'''
    try:
        result = subprocess.run(
            [*ssh_args_for_server(server, connect_timeout=8), "sh", "-s", "--", remote_path_for_capacity(server)],
            input=script,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            check=False,
            timeout=12,
            creationflags=create_no_window(),
        )
    except Exception:
        return {}
    if result.returncode != 0:
        return {}
    return parse_lustre_quota_kbytes(result.stdout)


def capacity_info(server: dict, rclone: str, status: str | None = None) -> dict:
    if (status or verified_mount_status(server)) != "mounted":
        return {}
    lustre_capacity = lustre_project_capacity_info(server)
    if lustre_capacity:
        return lustre_capacity
    state = current_state(server)
    remote = state.get("remote") or rsshmount.remote_spec(remote_name(server), server.get("remote_path") or "")
    try:
        result = run([rclone, "--config", str(rsshmount.rclone_config_path()), "about", remote, "--json"], capture=True)
        data = json.loads(result.stdout or "{}")
    except Exception:
        return {}
    total = data.get("total")
    used = data.get("used")
    free = data.get("free")
    try:
        total = int(total) if total is not None else None
        used = int(used) if used is not None else None
        free = int(free) if free is not None else None
    except (TypeError, ValueError):
        return {}
    if total is None and used is not None and free is not None:
        total = used + free
    if used is None and total is not None and free is not None:
        used = max(total - free, 0)
    if not total or used is None:
        return {}
    percent = int(round((used / total) * 100))
    return {"used": max(used, 0), "total": total, "percent": max(0, min(percent, 100))}


def split_remote_path(remote_path: str) -> tuple[str, str]:
    path = (remote_path or "").strip()
    if not path or path == "~":
        return "$HOME", ""
    if path.startswith("/"):
        return "/", path[1:]
    return "$HOME", path


def compose_remote_path(base: str, suffix: str) -> str:
    suffix = (suffix or "").strip().replace("\\", "/").strip("/")
    if base == "/":
        return "/" + suffix if suffix else "/"
    return suffix


def home_mountpoint_label(lang: str) -> str:
    return tr_lang(lang, "home_mountpoint")


def mountpoint_choices(lang: str = "en") -> list[str]:
    if os.name != "nt":
        return ["Auto"]
    choices = ["Auto", home_mountpoint_label(lang)]
    for letter in "ZYXWVUTSRQPONMLKJIHGFED":
        drive = f"{letter}:"
        if not rsshmount.windows_drive_in_use(drive):
            choices.append(drive)
    return choices


def mountpoint_value_to_choice(value: str, lang: str) -> str:
    if value == HOME_MOUNTPOINT_VALUE:
        return home_mountpoint_label(lang)
    return value or "Auto"


def mountpoint_choice_to_value(value: str, lang: str) -> str:
    text = (value or "").strip()
    if not text or text.lower() == "auto":
        return ""
    if text == HOME_MOUNTPOINT_VALUE or text in {home_mountpoint_label("en"), home_mountpoint_label("zh"), home_mountpoint_label(lang)}:
        return HOME_MOUNTPOINT_VALUE
    return text


def is_custom_mountpoint(value: str) -> bool:
    text = (value or "").strip()
    return bool(text and text.lower() != "auto" and text != HOME_MOUNTPOINT_VALUE)


def is_absolute_or_home_path(value: str) -> bool:
    text = (value or "").strip()
    if text.startswith(("~/", "~\\")):
        return True
    return Path(text).expanduser().is_absolute()


def is_windows_mount_drive(value: str) -> bool:
    return os.name == "nt" and rsshmount.is_windows_drive(str(value).strip())


def validate_mountpoint_for_save(value: str) -> str:
    text = (value or "").strip()
    if not is_custom_mountpoint(text):
        return ""
    if is_windows_mount_drive(text):
        return ""
    if not is_absolute_or_home_path(text):
        return "custom mountpoint must be an absolute path or start with ~"
    return ""


def validate_mountpoint_for_mount(mountpoint: str) -> str:
    value = str(mountpoint or "").strip()
    if not value:
        return "mountpoint is empty"
    if os.name == "nt":
        if value == "*" or rsshmount.is_windows_drive(value):
            if rsshmount.is_windows_drive(value) and rsshmount.windows_drive_in_use(value):
                return f"Windows drive is already in use: {value}"
            return ""
        path = Path(value).expanduser()
        if not path.is_absolute():
            return "Windows folder mountpoint must be an absolute path"
        if not path.parent.exists():
            return f"Windows folder mountpoint parent does not exist: {path.parent}"
        if path.exists():
            return f"Windows folder mountpoint target must not already exist: {path}"
        return ""

    path = Path(value).expanduser()
    if not path.is_absolute():
        return "custom mountpoint must be an absolute path or start with ~"
    if path.exists() and not path.is_dir():
        return f"mountpoint exists but is not a folder: {path}"
    if path.is_mount():
        return f"mountpoint is already mounted: {path}"
    return ""


def ssh_config_defaults(host_alias: str, config_path: str | Path | None = None) -> dict:
    if not host_alias:
        return {}
    config = rsshmount.read_ssh_config(host_alias, str(config_path) if config_path else None)
    key_file = rsshmount.first_usable_path(config.get("identityfile", []), must_exist=True)
    return {
        "name": host_alias,
        "host_alias": host_alias,
        "host": rsshmount.first_ssh_value(config, "hostname", host_alias),
        "user": rsshmount.first_ssh_value(config, "user", ""),
        "port": rsshmount.first_ssh_value(config, "port", "22"),
        "key_file": key_file,
    }


def sai_profile_name(user: str) -> str:
    user = str(user or "").strip()
    return f"SAI-{user}" if user else "SAI"


def sai_cluster_defaults(user: str = "") -> dict:
    name = sai_profile_name(user)
    return {
        "name": name,
        "host_alias": name,
        "host": "c1.sai.ai-4s.com",
        "user": str(user or "").strip(),
        "port": "12022",
        "auth": "key",
        "key_file": "",
        "connection_method": "native",
        "remote_path": "",
        "mountpoint": "",
        "ssh_config_managed": True,
        "copy_key_to_ssh_dir": True,
    }


def server_from_ssh_config_host(host_alias: str, config_path: str | Path | None = None) -> dict:
    defaults = ssh_config_defaults(host_alias, config_path)
    name = defaults.get("name") or host_alias
    return {
        "id": sanitize_server_id(name),
        "name": name,
        "mode": "manual",
        "source": "ssh_config",
        "host_alias": host_alias,
        "host": defaults.get("host", host_alias),
        "user": defaults.get("user", ""),
        "port": defaults.get("port") or "22",
        "auth": "key",
        "key_file": defaults.get("key_file", ""),
        "connection_method": "native",
        "remote_path": "",
        "mountpoint": "",
        "cache_mode": "",
    }


def normalized_target_port(value) -> str:
    text = str(value or "22").strip()
    return str(int(text)) if text.isdigit() else text


def normalized_host_alias(server: dict) -> str:
    return str(server.get("host_alias") or "").strip().casefold()


def target_fingerprint(server: dict) -> tuple[str, str, str]:
    return (
        str(server.get("host") or "").strip().casefold(),
        str(server.get("user") or "").strip(),
        normalized_target_port(server.get("port")),
    )


def exact_connection_fingerprint(server: dict) -> tuple[str, str, str]:
    return (
        str(server.get("host") or "").strip(),
        str(server.get("user") or "").strip(),
        normalized_target_port(server.get("port")),
    )


def full_batch_fingerprint(server: dict) -> tuple:
    return (
        normalized_host_alias(server),
        *target_fingerprint(server),
    )


def batch_duplicate_reason(server: dict, known: list[dict]) -> str:
    server_full = full_batch_fingerprint(server)
    server_alias = normalized_host_alias(server)
    server_target = target_fingerprint(server)
    for existing in known:
        if server_full == full_batch_fingerprint(existing):
            return "SAME"
    if server_alias:
        for existing in known:
            if server_alias == normalized_host_alias(existing):
                return "SAME HOST"
    for existing in known:
        if server_target == target_fingerprint(existing):
            return "SAME TARGET"
    return ""


def batch_duplicate_match(server: dict, known: list[dict]) -> tuple[str, dict | None]:
    server_full = full_batch_fingerprint(server)
    server_alias = normalized_host_alias(server)
    server_target = target_fingerprint(server)
    for existing in known:
        if server_full == full_batch_fingerprint(existing):
            return "SAME", existing
    if server_alias:
        for existing in known:
            if server_alias == normalized_host_alias(existing):
                return "SAME HOST", existing
    for existing in known:
        if server_target == target_fingerprint(existing):
            return "SAME TARGET", existing
    return "", None


def merge_batch_overwrite(existing: dict, imported: dict) -> dict:
    merged = dict(existing)
    for key in ("name", "mode", "source", "host_alias", "host", "user", "port", "auth", "key_file", "connection_method"):
        merged[key] = imported.get(key, "")
    if not merged.get("id"):
        merged["id"] = sanitize_server_id(merged.get("name") or merged.get("host_alias") or merged.get("host") or "")
    return merged


def ssh_config_batch_plan(config_path: str | Path, existing_servers: list[dict] | None = None) -> dict:
    path = Path(config_path).expanduser()
    hosts = list_ssh_config_hosts(path)
    existing = [dict(server, __batch_existing=True) for server in (existing_servers or [])]
    accepted: list[dict] = []
    skipped: list[dict] = []
    errors: list[dict] = []
    items: list[dict] = []
    statuses: dict[str, dict] = {}
    for host_alias in hosts:
        try:
            server = server_from_ssh_config_host(host_alias, path)
        except Exception as exc:
            item = {"host": host_alias, "status": "INVALID", "reason": str(exc)}
            errors.append(item)
            items.append(item)
            statuses[host_alias] = item
            continue
        if not server.get("host") or not server.get("user"):
            item = {"host": host_alias, "status": "INVALID", "reason": "missing HostName or User"}
            errors.append(item)
            items.append(item)
            statuses[host_alias] = item
            continue
        reason, matched = batch_duplicate_match(server, [*existing, *accepted])
        if reason:
            can_overwrite = bool(matched and matched.get("__batch_existing"))
            if matched:
                matched = {key: value for key, value in matched.items() if key != "__batch_existing"}
            item = {
                "host": host_alias,
                "status": reason,
                "reason": duplicate_reason_text(reason),
                "server": server,
                "match": matched,
                "can_overwrite": can_overwrite,
            }
            skipped.append(item)
            items.append(item)
            statuses[host_alias] = item
            continue
        accepted.append(server)
        item = {"host": host_alias, "status": "NEW", "reason": "", "server": server, "match": None, "can_overwrite": False}
        items.append(item)
        statuses[host_alias] = item
    return {"servers": accepted, "skipped": skipped, "errors": errors, "statuses": statuses, "hosts": hosts, "items": items}


def duplicate_reason_text(status: str) -> str:
    return {
        "SAME": "same config already exists",
        "SAME HOST": "same SSH Host already exists",
        "SAME TARGET": "same HostName/User/Port already exists",
        "INVALID": "invalid config",
    }.get(status, "")


def ssh_config_batch_servers(config_path: str | Path, existing_servers: list[dict] | None = None) -> tuple[list[dict], list[str]]:
    plan = ssh_config_batch_plan(config_path, existing_servers)
    messages = [f"{item['host']}: {item['status']} {item['reason']}".strip() for item in [*plan["skipped"], *plan["errors"]]]
    return plan["servers"], messages


def annotated_ssh_config_preview(config_path: str | Path, existing_servers: list[dict] | None = None, plan: dict | None = None) -> str:
    path = Path(config_path).expanduser()
    if plan is None:
        plan = ssh_config_batch_plan(path, existing_servers)
    statuses = plan["statuses"]
    entries = [entry for entry in list_ssh_config_host_entries(path) if Path(entry["path"]).resolve() == path.resolve()]
    line_annotations: dict[int, list[str]] = {}
    for entry in entries:
        item = statuses.get(entry["host"])
        if not item:
            continue
        label = item["status"]
        if label != "NEW":
            label = f"SKIP {label}"
        line_annotations.setdefault(int(entry["line"]), []).append(f"{entry['host']}: {label}")

    lines = []
    try:
        raw_lines = path.read_text(encoding="utf-8", errors="ignore").splitlines()
    except OSError as exc:
        raw_lines = [str(exc)]
    for line_no, raw_line in enumerate(raw_lines, 1):
        annotations = line_annotations.get(line_no)
        if annotations:
            lines.append(f"{raw_line}    # SSH MountMate: {'; '.join(annotations)}")
        else:
            lines.append(raw_line)

    summary = [
        f"NEW: {len(plan['servers'])}",
        f"SKIPPED: {len(plan['skipped'])}",
        f"INVALID: {len(plan['errors'])}",
        "",
    ]
    for item in plan["statuses"].values():
        status = item["status"]
        reason = item.get("reason") or ""
        suffix = f" - {reason}" if reason else ""
        summary.append(f"{status:<11} {item['host']}{suffix}")
    summary.extend(["", "----- SSH config preview -----", ""])
    return "\n".join([*summary, *lines])


def winfsp_installed() -> bool:
    return rsshmount.find_winfsp() is not None


def macfuse_installed() -> bool:
    if sys.platform != "darwin":
        return False
    candidates = [
        Path("/Library/Filesystems/macfuse.fs"),
        Path("/Library/Filesystems/osxfuse.fs"),
        Path("/usr/local/lib/libfuse.dylib"),
        Path("/opt/homebrew/lib/libfuse.dylib"),
    ]
    if any(path.exists() for path in candidates):
        return True
    try:
        result = subprocess.run(
            ["pkgutil", "--pkgs"],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            check=False,
            timeout=3,
        )
    except Exception:
        return False
    return any("macfuse" in line.lower() or "osxfuse" in line.lower() for line in result.stdout.splitlines())


def linux_fuse_installed() -> bool:
    if sys.platform != "linux":
        return False
    return (shutil.which("fusermount3") is not None or shutil.which("fusermount") is not None) and Path("/dev/fuse").exists()


def mount_dependency_label() -> str:
    if os.name == "nt":
        return "WinFsp"
    if sys.platform == "darwin":
        return "macFUSE"
    if sys.platform == "linux":
        return "FUSE"
    return "Mount deps"


def mount_dependency_installed() -> bool:
    if os.name == "nt":
        return winfsp_installed()
    if sys.platform == "darwin":
        return macfuse_installed()
    if sys.platform == "linux":
        return linux_fuse_installed()
    return True


def ssh_installed() -> bool:
    return shutil.which("ssh") is not None


def ps_quote(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


def run_visible_winget_install(title: str, package_id: str) -> tuple[int, Path]:
    app_dir().mkdir(parents=True, exist_ok=True)
    script = app_dir() / f"install-{package_id.replace('.', '-')}.cmd"
    ps_script = app_dir() / f"install-{package_id.replace('.', '-')}.ps1"
    log_path = app_dir() / f"install-{package_id.replace('.', '-')}.log"
    log_path.write_text(
        "\n".join(
            [
                "==== SSH MountMate installer launcher ====",
                f"Package: {package_id}",
                f"Batch script: {script}",
                f"PowerShell script: {ps_script}",
                "",
            ]
        ),
        encoding="utf-8-sig",
    )
    ps_script.write_text(
        "\n".join(
            [
                "$ErrorActionPreference = 'Continue'",
                "[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)",
                "$OutputEncoding = [System.Text.UTF8Encoding]::new($false)",
                "$PSDefaultParameterValues['Out-File:Encoding'] = 'utf8'",
                f"$LogPath = {ps_quote(str(log_path))}",
                "New-Item -ItemType Directory -Force -Path (Split-Path -Parent $LogPath) | Out-Null",
                "function Write-InstallLog([string]$Message) {",
                "  Add-Content -Path $LogPath -Value $Message -Encoding UTF8",
                "  Write-Host $Message",
                "}",
                'Write-InstallLog ""',
                'Write-InstallLog "==== SSH MountMate dependency install ===="',
                'Write-InstallLog ("Started: " + (Get-Date -Format o))',
                f'Write-InstallLog "Package: {package_id}"',
                'Write-InstallLog ("Log: " + $LogPath)',
                "$Winget = Get-Command winget.exe -ErrorAction SilentlyContinue",
                "if (-not $Winget) {",
                '  Write-InstallLog "winget.exe was not found in PATH."',
                '  Write-InstallLog "Install App Installer from Microsoft Store, then retry."',
                '  Write-InstallLog ""',
                "  exit 9009",
                "}",
                'Write-InstallLog ("winget: " + $Winget.Source)',
                'Write-InstallLog ""',
                f'& winget install --id "{package_id}" -e --accept-package-agreements --accept-source-agreements 2>&1 | ForEach-Object {{ Write-InstallLog ($_.ToString()) }}',
                "$RC = $LASTEXITCODE",
                'Write-InstallLog ""',
                'Write-InstallLog ("Finished: " + (Get-Date -Format o))',
                'Write-InstallLog ("Exit code: " + $RC)',
                "if ($RC -eq 0) {",
                '  Write-InstallLog "Installation command completed successfully."',
                "  exit 0",
                "}",
                'Write-InstallLog "Installation failed."',
                "exit $RC",
            ]
        ),
        encoding="utf-8-sig",
    )
    script.write_text(
        "\n".join(
            [
                "@echo off",
                "chcp 65001 >nul",
                f"title SSH MountMate - {title}",
                f'>> "{log_path}" echo ==== SSH MountMate installer wrapper ====',
                f'>> "{log_path}" echo Started: %DATE% %TIME%',
                f'>> "{log_path}" echo Package: {package_id}',
                f'>> "{log_path}" echo PowerShell script: "{ps_script}"',
                f'>> "{log_path}" echo.',
                f'echo Log file: "{log_path}"',
                "echo.",
                f'where powershell.exe >> "{log_path}" 2>&1',
                f'>> "{log_path}" echo Launching PowerShell installer...',
                f'powershell.exe -NoProfile -ExecutionPolicy Bypass -File "{ps_script}"',
                "set RC=%ERRORLEVEL%",
                f'>> "{log_path}" echo Wrapper exit code: %RC%',
                'if "%RC%"=="0" (',
                "  echo.",
                "  echo Installation command completed. This window will close in 5 seconds...",
                f'  timeout /t 5 /nobreak >> "{log_path}"',
                "  exit 0",
                ")",
                "echo.",
                "echo Installation failed or could not start. Exit code: %RC%",
                f'echo Log file: "{log_path}"',
                "echo This command window is intentionally left open for troubleshooting.",
            ]
        ),
        encoding="utf-8-sig",
    )
    result = subprocess.run(
        ["cmd.exe", "/k", str(script)],
        creationflags=getattr(subprocess, "CREATE_NEW_CONSOLE", 0),
        check=False,
    )
    return result.returncode, log_path


def install_rclone() -> None:
    if resolve_rclone_path():
        return
    try:
        install_managed_rclone()
    except Exception as exc:
        if os.name != "nt":
            raise RuntimeError(f"Automatic rclone download failed: {exc}\n\nInstall rclone manually and retry.\n\n" + manual_install_text()) from exc
    if resolve_rclone_path():
        return
    if os.name != "nt":
        raise RuntimeError("rclone is missing after automatic download.\n\nInstall rclone manually and retry.\n\n" + manual_install_text())
    code, log_path = run_visible_winget_install("rclone", "Rclone.Rclone")
    refresh_windows_path_env()
    if resolve_rclone_path():
        return
    raise RuntimeError(f"rclone was not found after winget finished. winget exit code: {code}. Log: {log_path}")


def install_winfsp() -> None:
    if winfsp_installed():
        return
    if os.name != "nt":
        raise RuntimeError("WinFsp is only required on Windows.")
    code, log_path = run_visible_winget_install("WinFsp", "WinFsp.WinFsp")
    refresh_windows_path_env()
    if winfsp_installed():
        return
    raise RuntimeError(
        f"WinFsp was not found after winget finished. winget exit code: {code}. Log: {log_path}\n\n"
        + manual_install_text(["WinFsp"])
    )


def install_openssh_client() -> None:
    command = (
        "Start-Process powershell -Verb RunAs "
        "-ArgumentList '-NoProfile -ExecutionPolicy Bypass -Command "
        "\"Add-WindowsCapability -Online -Name OpenSSH.Client~~~~0.0.1.0\"'"
    )
    run(["powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", command])


def obscure_password(rclone: str, password: str) -> str:
    result = run([rclone, "obscure", password], capture=True)
    return result.stdout.strip()


def ssh_command_for_server(server: dict) -> str:
    return " ".join(shlex.quote(part) for part in ssh_args_for_server(server))


def ssh_args_for_server(server: dict, *, connect_timeout: int | None = None) -> list[str]:
    parts = ["ssh", "-o", "BatchMode=yes"]
    if connect_timeout:
        parts.extend(["-o", f"ConnectTimeout={connect_timeout}"])
    if (server.get("source") == "ssh_config" or server.get("ssh_config_managed")) and server.get("host_alias"):
        parts.append(str(server["host_alias"]))
        return parts

    port = str(server.get("port") or "22")
    user = str(server.get("user") or "")
    key_file = str(server.get("key_file") or "")
    if user:
        parts.extend(["-l", user])
    if port:
        parts.extend(["-p", port])
    if key_file:
        parts.extend(["-i", key_file, "-o", "IdentitiesOnly=yes"])
    parts.append(str(server.get("host") or ""))
    return parts


def write_manual_remote(server: dict, rclone: str) -> None:
    with RCLONE_CONFIG_LOCK:
        with rsshmount.rclone_config_file_lock():
            write_manual_remote_unlocked(server, rclone)


def write_manual_remote_unlocked(server: dict, rclone: str) -> None:
    import configparser

    conf_path = rsshmount.rclone_config_path()
    conf_path.parent.mkdir(parents=True, exist_ok=True)
    parser = configparser.RawConfigParser()
    parser.optionxform = str
    parser.read(conf_path)

    remote = server["id"]
    if parser.has_section(remote):
        parser.remove_section(remote)
    parser.add_section(remote)
    parser.set(remote, "type", "sftp")
    parser.set(remote, "shell_type", "unix")
    parser.set(remote, "disable_hashcheck", "true")

    if connection_method_value(server) == "openssh":
        parser.set(remote, "ssh", ssh_command_for_server(server))
    else:
        parser.set(remote, "host", server["host"])
        parser.set(remote, "user", server["user"])
        parser.set(remote, "port", str(server.get("port") or "22"))

        if server.get("auth") == "password":
            parser.set(remote, "pass", server["password_obscured"])
        elif server.get("key_file"):
            parser.set(remote, "key_file", server["key_file"])
            if server.get("key_pass_obscured"):
                parser.set(remote, "key_file_pass", server["key_pass_obscured"])
        else:
            parser.set(remote, "key_use_agent", "true")

        known_hosts = rsshmount.update_app_known_hosts(server["host"], server.get("port") or "22") or rsshmount.default_known_hosts_file()
        if known_hosts.exists():
            parser.set(remote, "known_hosts_file", str(known_hosts))

    with conf_path.open("w", encoding="utf-8") as fh:
        parser.write(fh)


def ensure_remote(server: dict, rclone: str) -> None:
    with RCLONE_CONFIG_LOCK:
        if connection_method_value(server) == "openssh":
            write_manual_remote(server, rclone)
        elif server["mode"] == "ssh_config":
            rsshmount.ensure_rclone_remote(server["host_alias"], None, "auto")
        else:
            write_manual_remote(server, rclone)


def remote_name(server: dict) -> str:
    return server_remote_name_for_state(server)


def mount_server(server: dict, rclone: str, *, verify_existing: bool = True) -> dict:
    with rsshmount.server_operation_file_lock(server["id"]):
        return mount_server_locked(server, rclone, verify_existing=verify_existing)


def mount_server_locked(server: dict, rclone: str, *, verify_existing: bool = True) -> dict:
    if verify_existing and verified_mount_status(server) == "mounted":
        raise RuntimeError("This config is already mounted. Unmount it before mounting again.")
    ensure_remote(server, rclone)
    settings = load_settings()
    state_dir = rsshmount.app_state_dir()
    cache_dir = configured_cache_dir(remote_name(server))
    state_dir.mkdir(parents=True, exist_ok=True)
    cache_dir.mkdir(parents=True, exist_ok=True)

    remote_path = server.get("remote_path") or ""
    mountpoint = resolve_mountpoint(server)
    prepare_gui_mountpoint(mountpoint, home_mountpoint=server.get("mountpoint") == HOME_MOUNTPOINT_VALUE)
    remote = rsshmount.remote_spec(remote_name(server), remote_path)
    log_path = state_dir / f"{remote_name(server)}.log"
    rc_addr = f"127.0.0.1:{free_local_port()}"

    cmd = [
        rclone,
        "--config",
        str(rsshmount.rclone_config_path()),
        "--rc",
        "--rc-no-auth",
        "--rc-addr",
        rc_addr,
        "mount",
        remote,
        mountpoint,
        "--vfs-cache-mode",
        server.get("cache_mode") or settings.get("vfs_cache_mode", "writes"),
        "--vfs-fast-fingerprint",
        "--cache-dir",
        str(cache_dir),
        "--log-file",
        str(log_path),
        "--volname",
        server.get("name") or remote_name(server),
    ]
    if settings.get("vfs_cache_max_size"):
        cmd.extend(["--vfs-cache-max-size", settings["vfs_cache_max_size"]])
    if settings.get("vfs_cache_max_age"):
        cmd.extend(["--vfs-cache-max-age", settings["vfs_cache_max_age"]])
    if settings.get("vfs_cache_min_free_space"):
        cmd.extend(["--vfs-cache-min-free-space", settings["vfs_cache_min_free_space"]])
    if settings.get("vfs_write_back"):
        cmd.extend(["--vfs-write-back", settings["vfs_write_back"]])
    if settings.get("dir_cache_time"):
        cmd.extend(["--dir-cache-time", settings["dir_cache_time"]])
    if settings.get("buffer_size"):
        cmd.extend(["--buffer-size", settings["buffer_size"]])
    if server.get("network_mode") and os.name == "nt" and rsshmount.is_windows_drive(mountpoint):
        cmd.append("--network-mode")

    log = log_path.open("ab")
    flags = (
        getattr(subprocess, "DETACHED_PROCESS", 0)
        | getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
        | getattr(subprocess, "CREATE_NO_WINDOW", 0)
    )
    expected_state = {"remote": remote, "mountpoint": mountpoint, "log": str(log_path)}
    mountpoint_existed = mountpoint_ready(mountpoint)
    proc = subprocess.Popen(cmd, stdout=log, stderr=subprocess.STDOUT, creationflags=flags)
    wait_for_mount_ready(proc, mountpoint, log_path, expected_state, ready_before_start=mountpoint_existed)
    state = {"pid": proc.pid, "server_id": server["id"], "remote": remote, "mountpoint": mountpoint, "log": str(log_path), "rc_addr": rc_addr}
    (state_dir / f"{server['id']}.json").write_text(json.dumps(state, indent=2), encoding="utf-8")
    return state


def unmount_server(server: dict) -> None:
    with rsshmount.server_operation_file_lock(server["id"]):
        unmount_server_locked(server)


def unmount_server_locked(server: dict) -> None:
    state_file = rsshmount.app_state_dir() / f"{server['id']}.json"
    if not state_file.exists():
        raise RuntimeError("This server is not recorded as mounted.")
    state = json.loads(state_file.read_text(encoding="utf-8"))
    pid_int = int(state["pid"])
    pid = str(pid_int)
    if os.name == "nt":
        command = running_rclone_processes().get(int(pid), "")
        if not command:
            state_file.unlink(missing_ok=True)
            return
        if not command_matches_state(command, state):
            state_file.unlink(missing_ok=True)
            raise RuntimeError("Recorded PID no longer belongs to this mount. Removed stale state; the current rclone process was not stopped.")
        result = subprocess.run(["taskkill", "/PID", pid, "/T"], text=True, creationflags=create_no_window())
        if result.returncode != 0:
            run(["taskkill", "/PID", pid, "/T", "/F"])
        state_file.unlink(missing_ok=True)
        return

    if not pid_is_running(pid_int):
        state_file.unlink(missing_ok=True)
        return

    mountpoint = state.get("mountpoint") or current_mountpoint(server)
    errors: list[str] = []
    commands: list[list[str]]
    if sys.platform == "darwin":
        commands = [["umount", mountpoint], ["diskutil", "unmount", mountpoint], ["diskutil", "unmount", "force", mountpoint]]
    else:
        commands = []
        for tool in ("fusermount3", "fusermount"):
            if shutil.which(tool):
                commands.append([tool, "-u", mountpoint])
        commands.append(["umount", mountpoint])

    unmounted = False
    for command in commands:
        try:
            result = run(command, check=False, capture=True)
        except OSError as exc:
            errors.append(f"{command[0]}: {exc}")
            continue
        if result.returncode == 0:
            unmounted = True
            break
        output = "\n".join(part for part in [result.stdout, result.stderr] if part)
        errors.append(f"{' '.join(command)} exited {result.returncode}\n{output}".strip())

    if not unmounted:
        if sys.platform == "darwin":
            errors.append(
                "The mountpoint is still busy. Close files, terminals, editors, and Finder windows using it, then retry.\n"
                f"To inspect holders, run: lsof +D {shlex.quote(str(mountpoint))}"
            )
        raise RuntimeError("Failed to unmount mountpoint.\n" + "\n\n".join(errors))

    time.sleep(0.5)
    if pid_is_running(pid_int):
        try:
            os.kill(pid_int, 15)
            time.sleep(0.5)
            if pid_is_running(pid_int):
                os.kill(pid_int, 9)
        except OSError:
            pass
    state_file.unlink(missing_ok=True)


def refresh_remote_cache(server: dict, rclone: str) -> None:
    state = current_state(server)
    rc_addr = state.get("rc_addr")
    if not rc_addr:
        raise RuntimeError("This mount was created without rclone RC. Remount it before refreshing the directory cache.")
    run([rclone, "--rc-addr", rc_addr, "rc", "vfs/forget"], capture=True)


def startup_command(server_id: str) -> str:
    if getattr(sys, "frozen", False):
        return f'"{sys.executable}" --mount-id "{server_id}"'
    pythonw = shutil.which("pythonw.exe") or shutil.which("python.exe") or "python"
    return f'"{pythonw}" "{Path(__file__).resolve()}" --mount-id "{server_id}"'


def startup_supported() -> bool:
    return os.name == "nt" or sys.platform == "darwin"


def macos_startup_helper_path() -> Path:
    return user_data_dir() / "startup-helper" / MACOS_STARTUP_HELPER_NAME


def macos_startup_helper_version_path() -> Path:
    return macos_startup_helper_path().with_suffix(".version.json")


def macos_startup_helper_needs_update(source: Path, target: Path) -> bool:
    if not target.exists():
        return True
    try:
        source_stat = source.stat()
        target_stat = target.stat()
    except OSError:
        return True
    if source_stat.st_size != target_stat.st_size or source_stat.st_mtime > target_stat.st_mtime:
        return True
    try:
        marker = json.loads(macos_startup_helper_version_path().read_text(encoding="utf-8"))
    except Exception:
        return True
    return marker.get("version") != VERSION or marker.get("source") != str(source)


def install_macos_startup_helper() -> Path:
    if not getattr(sys, "frozen", False):
        return Path(sys.executable)
    source = Path(sys.executable).resolve()
    target = macos_startup_helper_path()
    if source == target:
        return target
    target.parent.mkdir(parents=True, exist_ok=True)
    if macos_startup_helper_needs_update(source, target):
        temp = target.with_name(f"{target.name}.tmp")
        shutil.copy2(source, temp)
        temp.chmod(temp.stat().st_mode | 0o111)
        temp.replace(target)
        macos_startup_helper_version_path().write_text(
            json.dumps(
                {
                    "version": VERSION,
                    "source": str(source),
                    "updated_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
                },
                indent=2,
            ),
            encoding="utf-8",
        )
    return target


def macos_startup_arguments(server_id: str) -> list[str]:
    if getattr(sys, "frozen", False):
        return [str(macos_startup_helper_path()), "--mount-id", server_id]
    return [sys.executable, "-m", "ssh_mountmate", "--mount-id", server_id]


def macos_launch_agent_label(server: dict) -> str:
    return f"com.sshmountmate.mount.{sanitize_server_id(str(server['id']))}"


def macos_launch_agent_path(server: dict) -> Path:
    return Path.home() / "Library" / "LaunchAgents" / f"{macos_launch_agent_label(server)}.plist"


def macos_launch_agent_plist(server: dict) -> dict:
    state_dir = rsshmount.app_state_dir()
    server_id = str(server["id"])
    return {
        "Label": macos_launch_agent_label(server),
        "ProgramArguments": macos_startup_arguments(server_id),
        "RunAtLoad": True,
        "KeepAlive": False,
        "ThrottleInterval": 60,
        "WorkingDirectory": str(Path.home()),
        "StandardOutPath": str(state_dir / f"{server_id}.startup.out.log"),
        "StandardErrorPath": str(state_dir / f"{server_id}.startup.err.log"),
    }


def enable_macos_startup(server: dict) -> None:
    path = macos_launch_agent_path(server)
    path.parent.mkdir(parents=True, exist_ok=True)
    rsshmount.app_state_dir().mkdir(parents=True, exist_ok=True)
    install_macos_startup_helper()
    with path.open("wb") as handle:
        plistlib.dump(macos_launch_agent_plist(server), handle, sort_keys=False)
    path.chmod(0o644)


def disable_macos_startup(server: dict) -> None:
    path = macos_launch_agent_path(server)
    if path.exists():
        try:
            subprocess.run(
                ["launchctl", "bootout", f"gui/{os.getuid()}", str(path)],
                check=False,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
        except Exception:
            pass
        path.unlink(missing_ok=True)


def startup_setup_log_path() -> Path:
    return rsshmount.app_state_dir() / "startup-setup.log"


def append_startup_setup_log(message: str) -> Path:
    path = startup_setup_log_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as handle:
        handle.write(f"==== {time.strftime('%Y-%m-%d %H:%M:%S')} ====\n")
        handle.write(message.rstrip() + "\n\n")
    return path


def enable_startup(server: dict) -> None:
    if sys.platform == "darwin":
        enable_macos_startup(server)
        return
    if os.name != "nt":
        return
    task_name = f"SSHMountMate-{server['id']}"
    run(["schtasks", "/Create", "/TN", task_name, "/SC", "ONLOGON", "/TR", startup_command(server["id"]), "/F"])


def disable_startup(server: dict) -> None:
    if sys.platform == "darwin":
        disable_macos_startup(server)
        return
    if os.name != "nt":
        return
    task_name = f"SSHMountMate-{server['id']}"
    subprocess.run(["schtasks", "/Delete", "/TN", task_name, "/F"], text=True, creationflags=create_no_window())


def headless_mount(server_id: str) -> int:
    servers = load_servers()
    server = next((item for item in servers if item.get("id") == server_id), None)
    if not server:
        return 2
    rclone = resolve_rclone_path()
    if not rclone:
        return 3
    state = mount_server(server, rclone)
    if sys.platform == "darwin":
        pid = int(state.get("pid") or 0)
        while pid and pid_is_running(pid):
            time.sleep(30)
    return 0


class Tooltip:
    def __init__(self, widget, text: str):
        self.widget = widget
        self.text = text
        self.tip = None
        widget.bind("<Enter>", self.show)
        widget.bind("<Leave>", self.hide)

    def show(self, _event=None) -> None:
        if self.tip or not self.text:
            return
        x = self.widget.winfo_rootx() + 18
        y = self.widget.winfo_rooty() + self.widget.winfo_height() + 8
        self.tip = Toplevel(self.widget)
        self.tip.wm_overrideredirect(True)
        self.tip.wm_geometry(f"+{x}+{y}")
        Label(self.tip, text=self.text, bg="#f7f7d0", fg="#222222", padx=6, pady=3, font=("Segoe UI", 9)).pack()

    def hide(self, _event=None) -> None:
        if self.tip:
            self.tip.destroy()
            self.tip = None


class App:
    def __init__(self, root: Tk):
        self.root = root
        self.root.title(APP_TITLE)
        self.root.geometry("760x460")
        self.root.minsize(560, 360)
        self.settings = load_settings()
        self.lang = effective_language(self.settings)
        configure_default_fonts(self.root, self.lang)
        self.servers: list[dict] = []
        self.rclone = ""

        self.status = StringVar(value=self.t("loading_configs"))
        self.dep_status = StringVar(value="")
        self.prompted_deps = False
        self.configs_loaded = False
        self.status_refreshing = False
        self.dependency_checking = False
        self.update_checking = False
        self.mount_status_cache: dict[str, str] = {}
        self.capacity_cache: dict[str, dict] = {}
        self.refresh_generation = 0
        self.card_action_columns = 4
        self.resize_refresh_pending = False
        self.batch_operation_running = False
        self.active_server_operations: set[str] = set()
        self.server_operation_lock = threading.Lock()
        self.mount_all_button = None
        self.unmount_all_button = None

        self.build()
        self.root.protocol("WM_DELETE_WINDOW", self.exit_app)
        self.refresh_list()
        self.root.after_idle(self.start_background_startup)

    def t(self, key: str, **kwargs) -> str:
        return tr_lang(self.lang, key, **kwargs)

    def status_text(self, status: str) -> str:
        return {
            "mounted": self.t("mounted_status"),
            "stopped": self.t("stopped_status"),
            "stale": self.t("stale_status"),
            "checking": self.t("checking_status"),
        }.get(status, status)

    def rebuild(self) -> None:
        for child in self.root.winfo_children():
            child.destroy()
        self.build()
        self.refresh_list()

    def start_background_startup(self) -> None:
        self.reload_configs_async()
        self.check_dependencies_async()

    def build(self) -> None:
        top = Frame(self.root, padx=10, pady=8)
        top.pack(fill=X)
        Label(top, text="ssh-mountmate").pack(side=LEFT)
        Button(top, text=self.t("settings"), command=self.open_settings).pack(side=RIGHT, padx=6)
        Button(top, text=self.t("add_config"), command=self.add_config).pack(side=RIGHT, padx=6)
        Button(top, text=self.t("refresh"), command=self.reload_configs_async).pack(side=RIGHT)

        body = Frame(self.root, padx=10, pady=4)
        body.pack(fill=BOTH, expand=True)

        self.cards_canvas = Canvas(body, bg="#202020", highlightthickness=0)
        self.cards_scrollbar = Scrollbar(body, orient="vertical", command=self.cards_canvas.yview)
        self.cards_frame = Frame(self.cards_canvas, bg="#202020")
        self.cards_window = self.cards_canvas.create_window((0, 0), window=self.cards_frame, anchor="nw")
        self.cards_frame.bind("<Configure>", lambda _event: self.cards_canvas.configure(scrollregion=self.cards_canvas.bbox("all")))
        self.cards_canvas.bind("<Configure>", self.on_cards_canvas_configure)
        self.cards_canvas.configure(yscrollcommand=self.cards_scrollbar.set)
        self.cards_canvas.pack(side=LEFT, fill=BOTH, expand=True)
        self.cards_scrollbar.pack(side=RIGHT, fill=Y)
        self.cards_canvas.bind("<MouseWheel>", self.on_cards_mousewheel)
        self.cards_canvas.bind("<Button-4>", self.on_cards_mousewheel)
        self.cards_canvas.bind("<Button-5>", self.on_cards_mousewheel)
        self.cards_frame.bind("<MouseWheel>", self.on_cards_mousewheel)
        self.cards_frame.bind("<Button-4>", self.on_cards_mousewheel)
        self.cards_frame.bind("<Button-5>", self.on_cards_mousewheel)

        bottom = Frame(self.root, padx=10, pady=8)
        bottom.pack(fill=X)
        Label(bottom, textvariable=self.status).pack(side=LEFT)
        self.unmount_all_button = Button(bottom, text=self.t("unmount_all"), command=self.unmount_all)
        self.unmount_all_button.pack(side=RIGHT, padx=(6, 0))
        self.mount_all_button = Button(bottom, text=self.t("mount_all"), command=self.mount_all)
        self.mount_all_button.pack(side=RIGHT)
        self.update_batch_buttons()

    def exit_app(self) -> None:
        self.root.destroy()

    def update_batch_buttons(self) -> None:
        state = "disabled" if self.batch_operation_running else "normal"
        for button in (self.mount_all_button, self.unmount_all_button):
            if button is not None:
                button.configure(state=state)

    def server_operation_id(self, server: dict) -> str:
        return server.get("id", "")

    def is_server_operation_active(self, server: dict) -> bool:
        server_id = self.server_operation_id(server)
        with self.server_operation_lock:
            return bool(server_id and server_id in self.active_server_operations)

    def claim_server_operation(self, server: dict) -> bool:
        server_id = self.server_operation_id(server)
        if not server_id:
            return False
        with self.server_operation_lock:
            if server_id in self.active_server_operations:
                return False
            self.active_server_operations.add(server_id)
            return True

    def release_server_operation(self, server: dict) -> None:
        server_id = self.server_operation_id(server)
        with self.server_operation_lock:
            self.active_server_operations.discard(server_id)

    def refresh_list(self) -> None:
        for child in self.cards_frame.winfo_children():
            child.destroy()
        if not self.configs_loaded:
            Label(
                self.cards_frame,
                text=self.t("loading_configs"),
                bg="#202020",
                fg="#bdbdbd",
                font=("Segoe UI", 11),
                pady=26,
            ).pack(fill=X)
            return
        if not self.servers:
            Label(
                self.cards_frame,
                text=self.t("no_configs"),
                bg="#202020",
                fg="#bdbdbd",
                font=("Segoe UI", 11),
                pady=26,
            ).pack(fill=X)
            return
        for server in self.servers:
            server_id = server.get("id", "")
            status = self.mount_status_cache.get(server_id, "checking")
            capacity = self.capacity_cache.get(server_id, {})
            self.add_server_card(server, status, capacity)
        self.bind_cards_mousewheel_recursive(self.cards_frame)

    def reload_configs_async(self) -> None:
        self.status.set(self.t("loading_configs"))

        def worker() -> None:
            servers = load_servers()
            self.root.after(0, lambda: self.apply_loaded_configs(servers))

        threading.Thread(target=worker, daemon=True).start()

    def apply_loaded_configs(self, servers: list[dict]) -> None:
        self.servers = servers
        self.configs_loaded = True
        self.status.set(self.t("ready"))
        self.refresh_list()
        self.refresh_mount_status_async()

    def refresh_mount_status_async(self) -> None:
        if self.status_refreshing:
            return
        self.status_refreshing = True
        self.refresh_generation += 1
        generation = self.refresh_generation
        servers = [dict(server) for server in self.servers]
        rclone = self.current_rclone()

        def worker() -> None:
            processes = running_rclone_processes() if os.name == "nt" else None
            statuses: dict[str, str] = {}
            for server in servers:
                server_id = server.get("id", "")
                if not server_id:
                    continue
                status = mount_status_with_processes(server, processes) if processes is not None else mount_status(server)
                statuses[server_id] = status
            self.root.after(0, lambda: self.apply_mount_statuses(generation, statuses))

            capacities: dict[str, dict] = {}
            for server in servers:
                server_id = server.get("id", "")
                if not server_id or statuses.get(server_id) != "mounted":
                    continue
                capacities[server_id] = capacity_info(server, rclone, statuses[server_id])
            self.root.after(0, lambda: self.apply_capacity_infos(generation, statuses, capacities))

        threading.Thread(target=worker, daemon=True).start()

    def apply_mount_statuses(self, generation: int, statuses: dict[str, str]) -> None:
        if generation != self.refresh_generation:
            return
        self.status_refreshing = False
        self.mount_status_cache = statuses
        for server_id, status in statuses.items():
            if status != "mounted":
                self.capacity_cache.pop(server_id, None)
        self.refresh_list()

    def apply_capacity_infos(self, generation: int, statuses: dict[str, str], capacities: dict[str, dict]) -> None:
        if generation != self.refresh_generation:
            return
        for server_id, status in statuses.items():
            if status == "mounted" and server_id in capacities:
                self.capacity_cache[server_id] = capacities[server_id]
            elif status != "mounted":
                self.capacity_cache.pop(server_id, None)
        self.refresh_list()

    def on_cards_mousewheel(self, event) -> None:
        if getattr(event, "num", None) == 4:
            direction = -1
        elif getattr(event, "num", None) == 5:
            direction = 1
        else:
            delta = getattr(event, "delta", 0)
            direction = -1 if delta > 0 else 1
        self.cards_canvas.yview_scroll(direction, "units")

    def action_button_columns_for_width(self, width: int | None = None) -> int:
        width = width or self.cards_canvas.winfo_width() or self.root.winfo_width()
        if width < 600:
            return 2
        return 4

    def on_cards_canvas_configure(self, event) -> None:
        self.cards_canvas.itemconfigure(self.cards_window, width=event.width)
        columns = self.action_button_columns_for_width(event.width)
        if columns == self.card_action_columns:
            return
        self.card_action_columns = columns
        if not self.resize_refresh_pending:
            self.resize_refresh_pending = True
            self.root.after(80, self.refresh_list_after_resize)

    def refresh_list_after_resize(self) -> None:
        self.resize_refresh_pending = False
        self.refresh_list()

    def bind_cards_mousewheel_recursive(self, widget) -> None:
        widget.bind("<MouseWheel>", self.on_cards_mousewheel)
        widget.bind("<Button-4>", self.on_cards_mousewheel)
        widget.bind("<Button-5>", self.on_cards_mousewheel)
        for child in widget.winfo_children():
            self.bind_cards_mousewheel_recursive(child)

    def add_server_card(self, server: dict, status: str = "checking", capacity: dict | None = None) -> None:
        mounted = status == "mounted"
        row_bg = "#2a2a2a" if mounted else "#242424"
        muted = "#7d7d7d"
        fg = "#f1f1f1" if mounted else "#bdbdbd"

        row = Frame(self.cards_frame, bg=row_bg, padx=12, pady=10)
        row.pack(fill=X, pady=4)

        left = Frame(row, bg=row_bg, width=90)
        left.pack(side=LEFT, fill="y")
        Label(left, text="🛡", bg=row_bg, fg=fg, font=("Segoe UI Emoji", 28)).pack(anchor="w")
        Label(left, text=self.status_text(status), bg=row_bg, fg=muted, font=(FONT_FAMILY_ZH if self.lang == "zh" else FONT_FAMILY_EN, 9)).pack(anchor="w", pady=(6, 0))

        actions = Frame(row, bg=row_bg)
        actions.pack(side=RIGHT, anchor="e")
        actions.grid_columnconfigure(0, minsize=42)
        actions.grid_columnconfigure(1, minsize=42)
        actions.grid_columnconfigure(2, minsize=42)
        actions.grid_columnconfigure(3, minsize=42)

        mid = Frame(row, bg=row_bg, padx=0)
        mid.pack(side=LEFT, fill=BOTH, expand=True, padx=(0, 18))
        drive = display_mountpoint_for_status(server, status)
        capacity = capacity or {}
        if mounted and capacity:
            capacity_label = self.t(
                "capacity_used",
                used=format_capacity_bytes(int(capacity["used"])),
                total=format_capacity_bytes(int(capacity["total"])),
                percent=int(capacity["percent"]),
            )
        else:
            capacity_label = self.t("checking_capacity") if mounted else self.t("unknown_capacity")
        font_family = FONT_FAMILY_ZH if self.lang == "zh" else FONT_FAMILY_EN
        text_width = max(18, min(64, (self.cards_canvas.winfo_width() - 260) // 8))
        Label(mid, text=f"{drive}  {server.get('name') or server.get('id')}", bg=row_bg, fg=fg, font=(font_family, 13, "bold"), anchor="w", width=text_width).pack(anchor="w", fill=X)
        Label(mid, text=capacity_label, bg=row_bg, fg="#c8c8c8", font=(font_family, 10), anchor="w", width=text_width).pack(anchor="w", fill=X)
        self.capacity_bar(mid, int(capacity.get("percent", 0)) if mounted and capacity else None, row_bg, muted).pack(fill=X, pady=(5, 4))
        Label(mid, text=f"{server.get('user', '')}@{server.get('host', '')}", bg=row_bg, fg=muted, font=(font_family, 10), anchor="e", width=text_width).pack(anchor="e", fill=X)
        Label(mid, text=server.get("remote_path") or "~", bg=row_bg, fg=muted, font=(font_family, 10), anchor="e", width=text_width).pack(anchor="e", fill=X)

        operation_active = self.is_server_operation_active(server)
        can_change_mount = not operation_active
        mount_tooltip = self.t("operation_busy") if operation_active else self.t("unmount") if mounted else self.t("mount")
        buttons = [
            ("■" if mounted else "▶", mount_tooltip, lambda s=server: self.toggle_mount(s), can_change_mount),
            ("📂", self.t("open_folder"), lambda s=server: self.open_folder(s), mounted and not operation_active),
            ("✎", self.t("edit_mounted_disabled") if mounted else self.t("edit_mount"), lambda s=server: self.edit_server(s), not mounted and can_change_mount and not self.batch_operation_running),
            ("🗑", self.t("delete_config"), lambda s=server: self.delete_server(s), not mounted and can_change_mount and not self.batch_operation_running),
        ]
        columns = self.card_action_columns
        for index, (text, tooltip, command, enabled) in enumerate(buttons):
            self.icon_button(actions, text, tooltip, command, enabled=enabled).grid(row=index // columns, column=index % columns, padx=2, pady=2)

    def icon_button(self, parent, text: str, tooltip: str, command, *, enabled: bool = True):
        button = Button(parent, text=text, width=3, height=1, command=command, font=("Segoe UI Emoji", 14))
        if not enabled:
            button.configure(fg="#777777", command=lambda: None)
        Tooltip(button, tooltip)
        return button

    def capacity_bar(self, parent, percent: int | None, bg: str, muted: str) -> Canvas:
        canvas = Canvas(parent, height=8, bg=bg, highlightthickness=0)

        def redraw(event=None) -> None:
            width = max(canvas.winfo_width(), 1)
            height = 8
            canvas.delete("all")
            canvas.create_rectangle(0, 0, width, height, fill="#3a3a3a", outline="")
            if percent is None:
                canvas.create_rectangle(0, 0, width, height, fill="#303030", outline="")
                return
            fill_width = int(width * max(0, min(percent, 100)) / 100)
            color = "#52b788" if percent < 80 else "#f0b429" if percent < 92 else "#e55353"
            canvas.create_rectangle(0, 0, fill_width, height, fill=color, outline="")
            canvas.create_line(0, height - 1, width, height - 1, fill=muted)

        canvas.bind("<Configure>", redraw)
        canvas.after_idle(redraw)
        return canvas

    def check_dependencies_async(self) -> None:
        if self.dependency_checking:
            return
        self.dependency_checking = True
        self.dep_status.set(self.t("checking_deps"))
        threading.Thread(target=self.check_dependencies, daemon=True).start()

    def check_dependencies(self) -> None:
        rclone_path = resolve_rclone_path()
        rclone_ok = bool(rclone_path)
        mount_dep = mount_dependency_label()
        mount_ok = mount_dependency_installed()
        ssh_ok = ssh_installed()
        missing = []
        if not rclone_ok:
            missing.append("rclone")
        if not mount_ok:
            missing.append(mount_dep)
        if not ssh_ok:
            missing.append("OpenSSH")
        self.root.after(0, lambda: self.apply_dependency_result(rclone_path, rclone_ok, mount_dep, mount_ok, ssh_ok, missing))

    def apply_dependency_result(self, rclone_path: str, rclone_ok: bool, mount_dep: str, mount_ok: bool, ssh_ok: bool, missing: list[str]) -> None:
        self.dependency_checking = False
        self.rclone = rclone_path
        self.dep_status.set(
            self.t(
                "deps_status",
                rclone=self.t("ok") if rclone_ok else self.t("missing"),
                mount_dep=mount_dep,
                mount=self.t("ok") if mount_ok else self.t("missing"),
                ssh=self.t("ok") if ssh_ok else self.t("missing"),
            )
        )
        if missing and not self.prompted_deps:
            self.prompted_deps = True
            self.prompt_install_deps(missing)

    def prompt_install_deps(self, missing: list[str]) -> None:
        if messagebox.askyesno(APP_TITLE, self.t("missing_dependencies", items=", ".join(missing))):
            self.install_deps_async()

    def open_settings(self) -> None:
        self.check_dependencies_async()
        settings = load_settings()
        window = Toplevel(self.root)
        window.title(self.t("settings"))
        window.geometry("560x700")
        frame = Frame(window, padx=14, pady=14)
        frame.pack(fill=BOTH, expand=True)
        Label(frame, textvariable=self.dep_status, anchor="w", justify=LEFT).pack(fill=X, pady=(0, 12))
        deps_check_button = Button(frame, text=self.t("check_dependencies"), command=self.check_dependencies_async)
        deps_check_button.pack(fill=X, pady=3)
        deps_install_button = Button(frame, text=self.t("install_missing_dependencies"), command=self.install_deps_async)
        deps_install_button.pack(fill=X, pady=3)
        updates_button = Button(frame, text=self.t("check_updates"), command=self.check_updates_async)
        updates_button.pack(fill=X, pady=3)
        logs_button = Button(frame, text=self.t("view_mount_logs"), command=self.open_logs)
        logs_button.pack(fill=X, pady=3)
        licenses_button = Button(frame, text=self.t("view_licenses"), command=lambda: self.show_text_window(self.t("view_licenses"), THIRD_PARTY_NOTICES))
        licenses_button.pack(fill=X, pady=3)

        ttk.Separator(frame).pack(fill=X, pady=12)

        cache_root = StringVar(value=settings.get("cache_root", default_settings()["cache_root"]))
        cache_mode = StringVar(value=settings.get("vfs_cache_mode", "writes"))
        cache_max_size = StringVar(value=setting_to_choice(settings.get("vfs_cache_max_size", ""), CACHE_SIZE_CHOICES[0]))
        cache_max_age = StringVar(value=setting_to_choice(settings.get("vfs_cache_max_age", ""), CACHE_AGE_CHOICES[0]))
        min_free_space = StringVar(value=setting_to_choice(settings.get("vfs_cache_min_free_space", ""), MIN_FREE_CHOICES[0]))
        write_back = StringVar(value=setting_to_choice(settings.get("vfs_write_back", ""), WRITE_BACK_CHOICES[0]))
        dir_cache_time = StringVar(value=setting_to_choice(settings.get("dir_cache_time", ""), DIR_CACHE_TIME_CHOICES[0]))
        buffer_size = StringVar(value=setting_to_choice(settings.get("buffer_size", ""), BUFFER_SIZE_CHOICES[0]))
        mount_workers = StringVar(value=setting_worker_choice(settings.get("mount_all_workers"), DEFAULT_MOUNT_ALL_WORKERS))
        unmount_workers = StringVar(value=setting_worker_choice(settings.get("unmount_all_workers"), DEFAULT_UNMOUNT_ALL_WORKERS))
        startup_all = BooleanVar(value=bool(settings.get("startup_all", False)) and startup_supported())
        language = StringVar(value=language_choice_from_setting(settings.get("language", "auto")))

        def attach_help(widget, key: str) -> None:
            Tooltip(widget, self.t(key))

        attach_help(deps_check_button, "dependency_help")
        attach_help(deps_install_button, "dependency_help")
        attach_help(updates_button, "updates_help")
        attach_help(logs_button, "logs_help")
        attach_help(licenses_button, "licenses_help")

        lang_row = Frame(frame)
        lang_row.pack(fill=X, pady=3)
        lang_label = Label(lang_row, text=self.t("language"), width=16, anchor="w")
        lang_label.pack(side=LEFT)
        language_combo = ttk.Combobox(lang_row, values=list(LANGUAGE_CHOICES.values()), textvariable=language, state="readonly")
        language_combo.pack(side=LEFT, fill=X, expand=True)
        attach_help(lang_label, "language_help")
        attach_help(language_combo, "language_help")

        cache_row = Frame(frame)
        cache_row.pack(fill=X, pady=3)
        cache_label = Label(cache_row, text=self.t("cache_root"), width=16, anchor="w")
        cache_label.pack(side=LEFT)
        cache_entry = Entry(cache_row, textvariable=cache_root)
        cache_entry.pack(side=LEFT, fill=X, expand=True)
        Button(cache_row, text="...", command=lambda: self.pick_cache_root(cache_root)).pack(side=RIGHT)
        attach_help(cache_label, "cache_root_help")
        attach_help(cache_entry, "cache_root_help")

        mode_row = Frame(frame)
        mode_row.pack(fill=X, pady=3)
        mode_label = Label(mode_row, text=self.t("vfs_cache_mode"), width=16, anchor="w")
        mode_label.pack(side=LEFT)
        mode_combo = ttk.Combobox(mode_row, values=["off", "minimal", "writes", "full"], textvariable=cache_mode, state="readonly")
        mode_combo.pack(side=LEFT, fill=X, expand=True)
        attach_help(mode_label, "vfs_cache_mode_help")
        attach_help(mode_combo, "vfs_cache_mode_help")

        size_row = Frame(frame)
        size_row.pack(fill=X, pady=3)
        size_label = Label(size_row, text=self.t("max_cache_size"), width=16, anchor="w")
        size_label.pack(side=LEFT)
        size_combo = ttk.Combobox(size_row, values=CACHE_SIZE_CHOICES, textvariable=cache_max_size, state="readonly")
        size_combo.pack(side=LEFT, fill=X, expand=True)
        attach_help(size_label, "max_cache_size_help")
        attach_help(size_combo, "max_cache_size_help")

        age_row = Frame(frame)
        age_row.pack(fill=X, pady=3)
        age_label = Label(age_row, text=self.t("max_cache_age"), width=16, anchor="w")
        age_label.pack(side=LEFT)
        age_combo = ttk.Combobox(age_row, values=CACHE_AGE_CHOICES, textvariable=cache_max_age, state="readonly")
        age_combo.pack(side=LEFT, fill=X, expand=True)
        attach_help(age_label, "max_cache_age_help")
        attach_help(age_combo, "max_cache_age_help")

        min_free_row = Frame(frame)
        min_free_row.pack(fill=X, pady=3)
        min_free_label = Label(min_free_row, text=self.t("min_free_space"), width=16, anchor="w")
        min_free_label.pack(side=LEFT)
        min_free_combo = ttk.Combobox(min_free_row, values=MIN_FREE_CHOICES, textvariable=min_free_space, state="readonly")
        min_free_combo.pack(side=LEFT, fill=X, expand=True)
        attach_help(min_free_label, "min_free_space_help")
        attach_help(min_free_combo, "min_free_space_help")

        write_back_row = Frame(frame)
        write_back_row.pack(fill=X, pady=3)
        write_back_label = Label(write_back_row, text=self.t("write_back"), width=16, anchor="w")
        write_back_label.pack(side=LEFT)
        write_back_combo = ttk.Combobox(write_back_row, values=WRITE_BACK_CHOICES, textvariable=write_back, state="readonly")
        write_back_combo.pack(side=LEFT, fill=X, expand=True)
        attach_help(write_back_label, "write_back_help")
        attach_help(write_back_combo, "write_back_help")

        dir_cache_row = Frame(frame)
        dir_cache_row.pack(fill=X, pady=3)
        dir_cache_label = Label(dir_cache_row, text=self.t("dir_cache_time"), width=16, anchor="w")
        dir_cache_label.pack(side=LEFT)
        dir_cache_combo = ttk.Combobox(dir_cache_row, values=DIR_CACHE_TIME_CHOICES, textvariable=dir_cache_time, state="readonly")
        dir_cache_combo.pack(side=LEFT, fill=X, expand=True)
        attach_help(dir_cache_label, "dir_cache_time_help")
        attach_help(dir_cache_combo, "dir_cache_time_help")

        buffer_row = Frame(frame)
        buffer_row.pack(fill=X, pady=3)
        buffer_label = Label(buffer_row, text=self.t("buffer_size"), width=16, anchor="w")
        buffer_label.pack(side=LEFT)
        buffer_combo = ttk.Combobox(buffer_row, values=BUFFER_SIZE_CHOICES, textvariable=buffer_size, state="readonly")
        buffer_combo.pack(side=LEFT, fill=X, expand=True)
        attach_help(buffer_label, "buffer_size_help")
        attach_help(buffer_combo, "buffer_size_help")

        mount_workers_row = Frame(frame)
        mount_workers_row.pack(fill=X, pady=3)
        mount_workers_label = Label(mount_workers_row, text=self.t("mount_workers"), width=16, anchor="w")
        mount_workers_label.pack(side=LEFT)
        mount_workers_combo = ttk.Combobox(mount_workers_row, values=BATCH_WORKER_CHOICES, textvariable=mount_workers, state="readonly")
        mount_workers_combo.pack(side=LEFT, fill=X, expand=True)
        attach_help(mount_workers_label, "mount_workers_help")
        attach_help(mount_workers_combo, "mount_workers_help")

        unmount_workers_row = Frame(frame)
        unmount_workers_row.pack(fill=X, pady=3)
        unmount_workers_label = Label(unmount_workers_row, text=self.t("unmount_workers"), width=16, anchor="w")
        unmount_workers_label.pack(side=LEFT)
        unmount_workers_combo = ttk.Combobox(unmount_workers_row, values=BATCH_WORKER_CHOICES, textvariable=unmount_workers, state="readonly")
        unmount_workers_combo.pack(side=LEFT, fill=X, expand=True)
        attach_help(unmount_workers_label, "unmount_workers_help")
        attach_help(unmount_workers_combo, "unmount_workers_help")

        startup_check = Checkbutton(
            frame,
            text=self.t("startup_all"),
            variable=startup_all,
            state="normal" if startup_supported() else "disabled",
        )
        startup_check.pack(anchor="w", pady=8)
        attach_help(startup_check, "startup_all_help")
        if sys.platform == "darwin":
            Label(frame, text=self.t("startup_all_macos_note"), fg="#666666", anchor="w").pack(fill=X, pady=(0, 6))

        def save() -> None:
            new_settings = load_settings()
            new_settings.update(
                {
                    "cache_root": cache_root.get().strip() or default_settings()["cache_root"],
                    "vfs_cache_mode": cache_mode.get() or "writes",
                    "vfs_cache_max_size": choice_to_setting(cache_max_size.get().strip()),
                    "vfs_cache_max_age": choice_to_setting(cache_max_age.get().strip()),
                    "vfs_cache_min_free_space": choice_to_setting(min_free_space.get().strip()),
                    "vfs_write_back": choice_to_setting(write_back.get().strip()),
                    "dir_cache_time": choice_to_setting(dir_cache_time.get().strip()),
                    "buffer_size": choice_to_setting(buffer_size.get().strip()),
                    "mount_all_workers": bounded_int(mount_workers.get(), DEFAULT_MOUNT_ALL_WORKERS),
                    "unmount_all_workers": bounded_int(unmount_workers.get(), DEFAULT_UNMOUNT_ALL_WORKERS),
                    "startup_all": bool(startup_all.get()) and startup_supported(),
                    "language": language_setting_from_choice(language.get()),
                }
            )
            save_settings(new_settings)
            old_lang = self.lang
            self.settings = new_settings
            self.lang = effective_language(new_settings)
            if self.lang != old_lang:
                configure_default_fonts(self.root, self.lang)
                self.rebuild()
            self.apply_startup_setting(new_settings["startup_all"])
            self.status.set(self.t("settings_saved"))
            window.destroy()

        Button(frame, text=self.t("save_settings"), command=save).pack(fill=X, pady=(12, 0))

    def pick_cache_root(self, variable: StringVar) -> None:
        path = filedialog.askdirectory(initialdir=variable.get() or str(Path.home()))
        if path:
            variable.set(path)

    def apply_startup_setting(self, enabled: bool) -> None:
        if not startup_supported():
            return
        errors: list[str] = []
        for server in self.servers:
            try:
                if enabled:
                    enable_startup(server)
                else:
                    disable_startup(server)
            except Exception as exc:
                errors.append(self.format_startup_error(server, enabled, exc))
        if errors:
            self.report_startup_errors(errors)

    def format_startup_error(self, server: dict, enabled: bool, exc: Exception) -> str:
        action = "enable" if enabled else "disable"
        name = server.get("name") or server.get("id") or "<unknown>"
        return (
            f"{name}: failed to {action} login mount\n"
            f"{exc}\n"
            f"{traceback.format_exc().rstrip()}"
        )

    def report_startup_errors(self, errors: list[str]) -> None:
        content = "\n\n".join(errors)
        try:
            log_path = append_startup_setup_log(content)
            message = self.t("startup_config_failed", path=log_path) + "\n\n" + content
        except Exception:
            message = content
        self.show_error(message)

    def install_deps_async(self) -> None:
        threading.Thread(target=self.install_deps, daemon=True).start()

    def install_deps(self) -> None:
        try:
            self.root.after(0, lambda: self.status.set(self.t("installing_deps")))
            if not resolve_rclone_path():
                install_rclone()
            if os.name == "nt" and not winfsp_installed():
                install_winfsp()
            if os.name == "nt" and not ssh_installed():
                install_openssh_client()
            if os.name != "nt":
                manual_missing = []
                if not mount_dependency_installed():
                    manual_missing.append(mount_dependency_label())
                if not ssh_installed():
                    manual_missing.append("OpenSSH")
                if manual_missing:
                    raise RuntimeError("Manual installation is required for: " + ", ".join(manual_missing) + "\n\n" + manual_install_text(manual_missing))
            rclone_path = resolve_rclone_path()
            self.root.after(0, lambda: self.on_dependency_install_done(rclone_path))
        except Exception as exc:
            message = str(exc)
            self.root.after(0, lambda: self.on_dependency_install_failed(message))

    def on_dependency_install_done(self, rclone_path: str) -> None:
        self.rclone = rclone_path
        self.status.set(self.t("deps_complete"))
        self.check_dependencies_async()

    def on_dependency_install_failed(self, message: str) -> None:
        self.status.set(self.t("deps_failed"))
        self.show_error(message)

    def check_updates_async(self) -> None:
        if self.update_checking:
            return
        self.update_checking = True
        self.status.set(self.t("checking_updates"))
        threading.Thread(target=self.check_updates, daemon=True).start()

    def check_updates(self) -> None:
        try:
            info = check_for_updates(VERSION)
            content = format_update_info(info, language=self.lang)
            self.root.after(0, lambda: self.on_update_check_done(content))
        except Exception as exc:
            message = f"{self.t('update_check_failed')}: {exc}"
            self.root.after(0, lambda: self.on_update_check_failed(message))

    def on_update_check_done(self, content: str) -> None:
        self.update_checking = False
        self.status.set(self.t("ready"))
        self.show_text_window(self.t("updates_title"), content)

    def on_update_check_failed(self, message: str) -> None:
        self.update_checking = False
        self.status.set(self.t("update_check_failed"))
        self.show_error(message)

    def show_text_window(self, title: str, content: str) -> None:
        window = Toplevel(self.root)
        window.title(title)
        window.geometry("720x420")
        frame = Frame(window, padx=10, pady=10)
        frame.pack(fill=BOTH, expand=True)
        scrollbar = Scrollbar(frame)
        text = Text(frame, wrap="word", yscrollcommand=scrollbar.set)
        scrollbar.configure(command=text.yview)
        text.insert("1.0", content)
        text.configure(state="disabled")
        text.pack(side=LEFT, fill=BOTH, expand=True)
        scrollbar.pack(side=RIGHT, fill=Y)
        buttons = Frame(window, padx=10, pady=8)
        buttons.pack(fill=X)

        def copy_content() -> None:
            self.root.clipboard_clear()
            self.root.clipboard_append(content)
            self.status.set(self.t("copied"))

        Button(buttons, text=self.t("copy"), command=copy_content).pack(side=RIGHT)
        Button(buttons, text=self.t("close"), command=window.destroy).pack(side=RIGHT, padx=6)

    def show_error(self, message: str) -> None:
        self.show_text_window(self.t("error_details"), message)

    def open_logs(self) -> None:
        if not self.servers:
            self.show_text_window(self.t("mount_log"), self.t("no_configs"))
            return
        window = Toplevel(self.root)
        window.title(self.t("view_mount_logs"))
        window.geometry("460x140")
        frame = Frame(window, padx=12, pady=12)
        frame.pack(fill=BOTH, expand=True)
        choices: dict[str, dict] = {}
        for index, server in enumerate(self.servers, 1):
            label = f"{server.get('name') or server.get('id')}  {display_mountpoint(server)}"
            if label in choices:
                label = f"{label}  #{index}"
            choices[label] = server
        selected = StringVar(value=next(iter(choices)))
        row = Frame(frame)
        row.pack(fill=X, pady=(0, 10))
        Label(row, text=self.t("select_log_config"), width=10, anchor="w").pack(side=LEFT)
        ttk.Combobox(row, values=list(choices), textvariable=selected, state="readonly").pack(side=LEFT, fill=X, expand=True)

        def open_selected() -> None:
            server = choices.get(selected.get())
            if server:
                window.destroy()
                self.open_log(server)

        Button(frame, text=self.t("view_log"), command=open_selected).pack(side=RIGHT)
        Button(frame, text=self.t("close"), command=window.destroy).pack(side=RIGHT, padx=6)

    def open_log(self, server: dict) -> None:
        path = current_log_path(server)
        try:
            lines = path.read_text(encoding="utf-8", errors="ignore").splitlines()
            content = f"{path}\n\n" + "\n".join(lines[-300:])
        except OSError as exc:
            content = f"{path}\n\n{exc}"
        self.show_text_window(self.t("mount_log"), content)

    def refresh_remote(self, server: dict) -> None:
        try:
            refresh_remote_cache(server, self.current_rclone())
            self.status.set(self.t("remote_refreshed"))
        except Exception as exc:
            self.show_error(str(exc))

    def batch_statuses(self, servers: list[dict]) -> dict[str, str]:
        processes = running_rclone_processes() if os.name == "nt" else None
        statuses: dict[str, str] = {}
        for server in servers:
            server_id = server.get("id", "")
            if not server_id:
                continue
            statuses[server_id] = mount_status_with_processes(server, processes) if processes is not None else mount_status(server)
        return statuses

    def finish_batch_operation(self, count: int, done: int, errors: list[str]) -> None:
        self.batch_operation_running = False
        self.update_batch_buttons()
        self.status.set(self.t("batch_complete", done=done, count=count))
        self.refresh_list()
        self.refresh_mount_status_async()
        if errors:
            self.show_error("\n\n".join(errors))

    def run_batch_operation(self, operation: str, workers: int, started_key: str) -> None:
        if self.batch_operation_running:
            self.status.set(self.t("batch_busy"))
            return
        servers = [dict(server) for server in self.servers]
        rclone = self.current_rclone()
        self.batch_operation_running = True
        self.update_batch_buttons()
        self.refresh_list()

        def worker() -> None:
            errors: list[str] = []
            done = 0
            count = 0

            try:
                statuses = self.batch_statuses(servers)
                if operation == "mount":
                    targets = [server for server in servers if statuses.get(server.get("id", "")) != "mounted"]
                else:
                    targets = [server for server in servers if statuses.get(server.get("id", "")) == "mounted"]
                targets = [server for server in targets if self.claim_server_operation(server)]
                count = len(targets)
                self.root.after(0, lambda: (self.status.set(self.t(started_key, count=count)), self.refresh_list()))

                def run_one(server: dict) -> None:
                    try:
                        if operation == "mount":
                            mount_server(server, rclone)
                        else:
                            unmount_server(server)
                    finally:
                        self.release_server_operation(server)

                if count:
                    max_workers = max(1, min(workers, count))
                    with ThreadPoolExecutor(max_workers=max_workers) as executor:
                        futures = {executor.submit(run_one, server): server for server in targets}
                        for future in as_completed(futures):
                            server = futures[future]
                            try:
                                future.result()
                                done += 1
                            except Exception as exc:
                                errors.append(f"{server.get('name') or server.get('id')}: {exc}")
            except Exception as exc:
                errors.append(str(exc))

            self.root.after(0, lambda: self.finish_batch_operation(count, done, errors))

        threading.Thread(target=worker, daemon=True).start()

    def mount_all(self) -> None:
        settings = load_settings()
        workers = bounded_int(settings.get("mount_all_workers"), DEFAULT_MOUNT_ALL_WORKERS)
        self.run_batch_operation("mount", workers, "mount_all_started")

    def unmount_all(self) -> None:
        settings = load_settings()
        workers = bounded_int(settings.get("unmount_all_workers"), DEFAULT_UNMOUNT_ALL_WORKERS)
        self.run_batch_operation("unmount", workers, "unmount_all_started")

    def finish_single_operation(self, server: dict, message: str = "", error: str = "") -> None:
        self.release_server_operation(server)
        if message:
            self.status.set(message)
        self.refresh_list()
        self.refresh_mount_status_async()
        if error:
            self.show_error(error)

    def add_config(self) -> None:
        dialog = ServerDialog(self.root, rclone=self.current_rclone(), lang=self.lang, existing_servers=self.servers)
        self.root.wait_window(dialog.window)
        if dialog.result:
            results = dialog.result if isinstance(dialog.result, list) else [dialog.result]
            used_ids = {server.get("id", "") for server in self.servers}
            used_names: set[str] = set()
            used_mount_folders: set[str] = set()
            for server in self.servers:
                add_used_server_name(server_name_base(server), used_names, used_mount_folders)
            changed_results: list[dict] = []
            for result in results:
                action = result.pop("__batch_action", "")
                target_id = result.pop("__batch_target_id", "")
                if action == "overwrite" and target_id:
                    for index, existing in enumerate(self.servers):
                        if str(existing.get("id") or "") == str(target_id):
                            merged = merge_batch_overwrite(existing, result)
                            used_names_without_target: set[str] = set()
                            used_mount_folders_without_target: set[str] = set()
                            for other_index, other in enumerate(self.servers):
                                if other_index != index:
                                    add_used_server_name(server_name_base(other), used_names_without_target, used_mount_folders_without_target)
                            merged["name"] = make_unique_server_name(server_name_base(merged), used_names_without_target, used_mount_folders_without_target)
                            self.servers[index] = merged
                            changed_results.append(merged)
                            break
                    continue
                result["name"] = make_unique_server_name(server_name_base(result), used_names, used_mount_folders)
                add_used_server_name(result["name"], used_names, used_mount_folders)
                result["id"] = make_unique_server_id(result.get("id") or result.get("name", ""), used_ids)
                used_ids.add(result["id"])
                self.servers.append(result)
                changed_results.append(result)
            save_servers(self.servers)
            if load_settings().get("startup_all"):
                startup_errors: list[str] = []
                for result in changed_results:
                    try:
                        enable_startup(result)
                    except Exception as exc:
                        startup_errors.append(self.format_startup_error(result, True, exc))
                if startup_errors:
                    self.report_startup_errors(startup_errors)
            if len(changed_results) > 1:
                self.status.set(self.t("imported_configs", count=len(changed_results)))
            self.refresh_list()
            self.refresh_mount_status_async()

    def edit_server(self, server: dict) -> None:
        if self.batch_operation_running or self.is_server_operation_active(server):
            self.status.set(self.t("operation_busy"))
            return
        try:
            index = self.servers.index(server)
        except ValueError:
            return
        dialog = ServerDialog(self.root, rclone=self.current_rclone(), existing=server, lang=self.lang, existing_servers=self.servers)
        self.root.wait_window(dialog.window)
        if dialog.result:
            old_server = dict(server)
            result = dialog.result
            used_names: set[str] = set()
            used_mount_folders: set[str] = set()
            for existing_index, existing_server in enumerate(self.servers):
                if existing_index != index:
                    add_used_server_name(server_name_base(existing_server), used_names, used_mount_folders)
            result["name"] = make_unique_server_name(server_name_base(result), used_names, used_mount_folders)
            if old_server.get("ssh_config_managed") and (
                not result.get("ssh_config_managed")
                or str(old_server.get("managed_ssh_config_path") or "") != str(result.get("managed_ssh_config_path") or "")
            ):
                remove_managed_ssh_config(old_server)
            self.servers[index] = result
            save_servers(self.servers)
            self.refresh_list()
            self.refresh_mount_status_async()

    def toggle_mount(self, server: dict) -> None:
        if not self.claim_server_operation(server):
            self.status.set(self.t("operation_busy"))
            return
        server = dict(server)
        cached_status = self.mount_status_cache.get(server.get("id", ""))
        name = server.get("name") or server.get("id")
        rclone = self.current_rclone()
        self.status.set(self.t("unmount_started" if cached_status == "mounted" else "mount_started", name=name))
        self.refresh_list()

        def worker() -> None:
            message = ""
            error = ""
            try:
                if verified_mount_status(server) == "mounted":
                    unmount_server(server)
                    message = self.t("unmounted")
                else:
                    state = mount_server(server, rclone)
                    message = self.t("mounted_at", remote=state["remote"], mountpoint=state["mountpoint"])
            except Exception as exc:
                error = str(exc)
            self.root.after(0, lambda: self.finish_single_operation(server, message, error))

        threading.Thread(target=worker, daemon=True).start()

    def open_folder(self, server: dict) -> None:
        if verified_mount_status(server) != "mounted":
            messagebox.showinfo(APP_TITLE, self.t("mount_before_open"))
            return
        mountpoint = current_mountpoint(server)
        try:
            if os.name == "nt":
                os.startfile(mountpoint)
            elif sys.platform == "darwin":
                subprocess.Popen(["open", mountpoint])
            else:
                subprocess.Popen(["xdg-open", mountpoint])
        except Exception as exc:
            self.show_error(str(exc))

    def delete_server(self, server: dict) -> None:
        if self.batch_operation_running or self.is_server_operation_active(server):
            self.status.set(self.t("operation_busy"))
            return
        status = verified_mount_status(server)
        name = server.get("name") or server.get("id")
        if status == "mounted":
            if not messagebox.askyesno(APP_TITLE, self.t("delete_mounted_confirm", name=name)):
                return
            try:
                unmount_server(server)
            except Exception as exc:
                self.show_error(str(exc))
                return
        else:
            if not messagebox.askyesno(APP_TITLE, self.t("delete_confirm", name=name)):
                return
        state_file = server_state_file(server)
        if state_file.exists() and verified_mount_status(server) != "mounted":
            state_file.unlink(missing_ok=True)
        self.servers = [item for item in self.servers if item is not server and item.get("id") != server.get("id")]
        try:
            disable_startup(server)
        except Exception:
            pass
        if server.get("ssh_config_managed"):
            remove_managed_ssh_config(server)
        save_servers(self.servers)
        self.status.set(self.t("deleted", name=name))
        self.refresh_list()
        self.refresh_mount_status_async()

    def current_rclone(self) -> str:
        if not self.rclone:
            self.rclone = resolve_rclone_path()
        return self.rclone


class ServerDialog:
    def __init__(self, root: Tk, *, rclone: str, lang: str, existing: dict | None = None, existing_servers: list[dict] | None = None):
        self.result = None
        self.rclone = rclone
        self.lang = lang
        self.existing = existing or {}
        self.existing_servers = [dict(server) for server in (existing_servers or [])]
        existing_source = self.existing.get("source")
        if not existing_source and self.existing.get("mode") == "ssh_config":
            existing_source = "ssh_config"
        self.source = StringVar(value=existing_source or "ssh_config")
        self.auth = StringVar(value=self.existing.get("auth", "key"))
        self.connection_method = StringVar(value=connection_method_value(self.existing))
        self.write_ssh_config = BooleanVar(value=bool(self.existing.get("ssh_config_managed", False)))
        self.copy_key_to_ssh = BooleanVar(value=bool(self.existing.get("copy_key_to_ssh_dir", False)))
        self.values: dict[str, Entry] = {}
        self.required_stars: dict[str, Label] = {}
        self.batch_conflict_actions: dict[str, tuple[dict, StringVar]] = {}
        self.last_sai_profile_name = sai_profile_name(self.existing.get("user", ""))
        self.batch_config_path = StringVar(value=str(Path.home() / ".ssh" / "config"))
        self.window = Toplevel(root)
        self.window.title(self.t("edit_config_title") if existing else self.t("add_config_title"))
        self.window.geometry("660x600")
        self.window.minsize(560, 420)
        self.window.resizable(True, True)
        self.buttons_frame = Frame(self.window, padx=10, pady=10)
        self.buttons_frame.pack(side="bottom", fill=X)
        self.content_frame = Frame(self.window)
        self.content_frame.pack(side="top", fill=BOTH, expand=True)
        self.canvas = Canvas(self.content_frame, highlightthickness=0)
        self.scrollbar = Scrollbar(self.content_frame, orient="vertical", command=self.canvas.yview)
        self.form = Frame(self.canvas)
        self.form.bind("<Configure>", lambda _event: self.update_scrollregion())
        self.form_window = self.canvas.create_window((0, 0), window=self.form, anchor="nw")
        self.canvas.bind("<Configure>", self.on_canvas_configure)
        self.canvas.configure(yscrollcommand=self.scrollbar.set)
        self.canvas.pack(side=LEFT, fill=BOTH, expand=True)
        self.scrollbar.pack(side=RIGHT, fill=Y)
        self.window.bind("<MouseWheel>", self.on_mousewheel)
        self.window.bind("<Button-4>", self.on_mousewheel)
        self.window.bind("<Button-5>", self.on_mousewheel)
        self.canvas.bind("<MouseWheel>", self.on_mousewheel)
        self.canvas.bind("<Button-4>", self.on_mousewheel)
        self.canvas.bind("<Button-5>", self.on_mousewheel)
        self.form.bind("<MouseWheel>", self.on_mousewheel)
        self.form.bind("<Button-4>", self.on_mousewheel)
        self.form.bind("<Button-5>", self.on_mousewheel)
        self.build()
        self.window.bind_all("<MouseWheel>", self.on_mousewheel)
        self.window.bind_all("<Button-4>", self.on_mousewheel)
        self.window.bind_all("<Button-5>", self.on_mousewheel)
        self.window.bind("<Destroy>", self.on_destroy)

    def t(self, key: str, **kwargs) -> str:
        return tr_lang(self.lang, key, **kwargs)

    def label_width(self, text: str) -> int:
        return max(FORM_LABEL_CHARS, len(str(text or "")) + (3 if text else 0))

    def label(self, parent, text: str, *, required: bool = False, required_key: str = "") -> None:
        label_frame = Frame(parent)
        label_frame.pack(side=LEFT, fill=Y, padx=(0, 8))
        Label(label_frame, text=text, width=self.label_width(text), anchor="w").pack(side=LEFT)
        if required or required_key:
            star = Label(label_frame, text="*", fg="#d32f2f", anchor="w")
            star.pack(side=LEFT, padx=(2, 0))
            if required_key:
                self.required_stars[required_key] = star
                if not required:
                    star.pack_forget()

    def row(self, label: str, key: str, default: str = "", browse=False, secret=False, parent=None, required: bool = False, required_key: str = ""):
        frame = Frame(parent or self.form, padx=10, pady=4)
        frame.pack(fill=X)
        self.label(frame, label, required=required, required_key=required_key)
        entry = Entry(frame, show="*" if secret else None)
        entry.insert(0, default)
        entry.pack(side=LEFT, fill=X, expand=True)
        self.values[key] = entry
        if browse:
            Button(frame, text="...", command=lambda: self.pick_file(key)).pack(side=RIGHT)
        return entry

    def row_combo(self, label: str, key: str, values: list[str], default: str = "", parent=None, required: bool = False):
        frame = Frame(parent or self.form, padx=10, pady=4)
        frame.pack(fill=X)
        self.label(frame, label, required=required)
        combo = ttk.Combobox(frame, values=values)
        if default:
            combo.set(default)
        elif values:
            combo.set(values[0])
        combo.pack(side=LEFT, fill=X, expand=True)
        self.values[key] = combo
        return combo

    def row_remote_path(self, remote_path: str, parent=None) -> None:
        base, suffix = split_remote_path(remote_path)
        frame = Frame(parent or self.form, padx=10, pady=4)
        frame.pack(fill=X)
        self.label(frame, self.t("remote_path"))
        combo = ttk.Combobox(frame, values=["$HOME", "/"], width=8, state="readonly")
        combo.set(base)
        combo.pack(side=LEFT)
        self.values["remote_base"] = combo
        entry = Entry(frame)
        entry.insert(0, suffix)
        entry.pack(side=LEFT, fill=X, expand=True, padx=(6, 0))
        self.values["remote_suffix"] = entry

    def build(self) -> None:
        source_frame = Frame(self.form, padx=10, pady=4)
        source_frame.pack(fill=X)
        self.label(source_frame, self.t("source"))
        source_options = Frame(source_frame)
        source_options.pack(side=LEFT, fill=X, expand=True)
        ttk.Radiobutton(source_options, text=self.t("ssh_config"), variable=self.source, value="ssh_config", command=self.on_source_changed).pack(side=LEFT, padx=(0, 12))
        if not self.existing:
            ttk.Radiobutton(source_options, text=self.t("ssh_config_batch"), variable=self.source, value="ssh_config_batch", command=self.on_source_changed).pack(side=LEFT, padx=(0, 12))
        ttk.Radiobutton(source_options, text=self.t("sai_cluster"), variable=self.source, value="sai_cluster", command=self.on_source_changed).pack(side=LEFT, padx=(0, 12))
        ttk.Radiobutton(source_options, text=self.t("manual"), variable=self.source, value="manual", command=self.on_source_changed).pack(side=LEFT, padx=(0, 12))

        self.single_frame = Frame(self.form)
        self.single_frame.pack(fill=X)

        hosts = list_ssh_config_hosts()
        host_default = self.existing.get("host_alias") or (hosts[0] if hosts else "")
        self.host_combo = self.row_combo(self.t("ssh_host"), "host_alias", hosts, host_default, parent=self.single_frame)
        self.host_combo.bind("<<ComboboxSelected>>", self.on_ssh_host_selected)

        self.row(self.t("name"), "name", self.existing.get("name", ""), parent=self.single_frame, required=True)
        self.row(self.t("ip_host"), "host", self.existing.get("host", ""), parent=self.single_frame, required=True)
        user_entry = self.row(self.t("user"), "user", self.existing.get("user", ""), parent=self.single_frame, required=True)
        user_entry.bind("<KeyRelease>", self.on_sai_user_changed)
        user_entry.bind("<FocusOut>", self.on_sai_user_changed)
        self.row(self.t("port"), "port", str(self.existing.get("port") or "22"), parent=self.single_frame, required=True)

        auth_frame = Frame(self.single_frame, padx=10, pady=4)
        auth_frame.pack(fill=X)
        self.label(auth_frame, self.t("auth"))
        self.auth_buttons = [
            ttk.Radiobutton(auth_frame, text=self.t("key"), variable=self.auth, value="key", command=self.update_connection_method_controls),
            ttk.Radiobutton(auth_frame, text=self.t("password_auth"), variable=self.auth, value="password", command=self.update_connection_method_controls),
        ]
        for button in self.auth_buttons:
            button.pack(side=LEFT)
        self.row(self.t("key_file"), "key_file", self.existing.get("key_file", ""), browse=True, parent=self.single_frame, required_key="key_file")
        self.row(self.t("key_passphrase"), "key_passphrase", secret=True, parent=self.single_frame)
        self.row(self.t("password"), "password", secret=True, parent=self.single_frame, required_key="password")

        ssh_write_frame = Frame(self.single_frame, padx=10, pady=4)
        ssh_write_frame.pack(fill=X)
        self.label(ssh_write_frame, "")
        self.write_ssh_config_check = Checkbutton(ssh_write_frame, text=self.t("write_ssh_config"), variable=self.write_ssh_config, command=self.update_source_controls)
        self.write_ssh_config_check.pack(side=LEFT)
        self.copy_key_check = Checkbutton(ssh_write_frame, text=self.t("copy_key_to_ssh_dir"), variable=self.copy_key_to_ssh, command=self.update_connection_method_controls)
        self.copy_key_check.pack(side=LEFT, padx=(12, 0))
        Tooltip(self.write_ssh_config_check, self.t("ssh_config_write_help"))
        Tooltip(self.copy_key_check, self.t("copy_key_help"))

        method_frame = Frame(self.single_frame, padx=10, pady=4)
        method_frame.pack(fill=X)
        self.label(method_frame, self.t("connection_method"))
        ttk.Radiobutton(method_frame, text=self.t("rclone_native"), variable=self.connection_method, value="native", command=self.update_connection_method_controls).pack(side=LEFT)
        ttk.Radiobutton(method_frame, text=self.t("openssh"), variable=self.connection_method, value="openssh", command=self.update_connection_method_controls).pack(side=LEFT)
        self.connection_help = Label(self.single_frame, text=self.t("openssh_help"), fg="#666666", wraplength=520, justify=LEFT)

        self.row_remote_path(self.existing.get("remote_path", ""), parent=self.single_frame)
        mountpoint_combo = self.row_combo(
            self.t("mountpoint"),
            "mountpoint",
            mountpoint_choices(self.lang),
            mountpoint_value_to_choice(self.existing.get("mountpoint", ""), self.lang),
            parent=self.single_frame,
        )
        Tooltip(mountpoint_combo, self.t("mountpoint_help"))

        self.build_batch_frame()

        self.save_button = Button(self.buttons_frame, text=self.t("save"), command=self.save)
        self.save_button.pack(side=RIGHT)
        Button(self.buttons_frame, text=self.t("cancel"), command=self.window.destroy).pack(side=RIGHT, padx=6)

        self.update_source_controls()
        self.update_connection_method_controls()
        if self.source.get() == "ssh_config" and not self.existing and host_default:
            self.apply_ssh_defaults(host_default)
        if self.source.get() == "sai_cluster" and not self.existing:
            self.apply_sai_defaults()
        self.bind_mousewheel_recursive(self.form)

    def build_batch_frame(self) -> None:
        self.batch_frame = Frame(self.form)

        file_row = Frame(self.batch_frame, padx=10, pady=4)
        file_row.pack(fill=X)
        self.label(file_row, self.t("ssh_config_file"))
        file_entry = Entry(file_row, textvariable=self.batch_config_path)
        file_entry.pack(side=LEFT, fill=X, expand=True)
        file_entry.bind("<Return>", lambda _event: self.load_batch_preview())
        file_entry.bind("<FocusOut>", lambda _event: self.load_batch_preview())
        Button(file_row, text=self.t("browse"), command=self.pick_batch_config).pack(side=RIGHT, padx=(6, 0))

        Label(self.batch_frame, text=self.t("preview"), anchor="w").pack(fill=X, padx=10, pady=(8, 2))
        preview_frame = Frame(self.batch_frame, padx=10)
        preview_frame.pack(fill=BOTH)
        preview_scroll = Scrollbar(preview_frame)
        self.batch_preview = Text(preview_frame, height=14, wrap="none", yscrollcommand=preview_scroll.set)
        preview_scroll.configure(command=self.batch_preview.yview)
        self.batch_preview.pack(side=LEFT, fill=BOTH, expand=True)
        preview_scroll.pack(side=RIGHT, fill=Y)
        self.batch_preview.bind("<MouseWheel>", self.on_batch_preview_mousewheel)
        self.batch_preview.bind("<Button-4>", self.on_batch_preview_mousewheel)
        self.batch_preview.bind("<Button-5>", self.on_batch_preview_mousewheel)

        self.batch_conflicts_frame = Frame(self.batch_frame)
        conflicts_header = Frame(self.batch_conflicts_frame)
        conflicts_header.pack(fill=X)
        Label(conflicts_header, text=self.t("batch_conflicts"), anchor="w").pack(side=LEFT)
        help_label = Label(conflicts_header, text="?", fg="#666666")
        help_label.pack(side=LEFT, padx=(6, 0))
        Tooltip(help_label, self.t("batch_overwrite_help"))
        self.batch_conflicts_body = Frame(self.batch_conflicts_frame)
        self.batch_conflicts_body.pack(fill=X)

        self.load_batch_preview()

    def on_batch_preview_mousewheel(self, event):
        if getattr(event, "num", None) == 4:
            direction = -1
        elif getattr(event, "num", None) == 5:
            direction = 1
        else:
            delta = getattr(event, "delta", 0)
            direction = -1 if delta > 0 else 1
        self.batch_preview.yview_scroll(direction, "units")
        return "break"

    def pick_batch_config(self) -> None:
        path = filedialog.askopenfilename(
            initialdir=str((Path.home() / ".ssh").expanduser()),
            initialfile="config",
        )
        if path:
            self.batch_config_path.set(path)
            self.load_batch_preview()

    def load_batch_preview(self) -> None:
        path = Path(self.batch_config_path.get()).expanduser()
        try:
            plan = ssh_config_batch_plan(path, self.existing_servers)
            content = annotated_ssh_config_preview(path, self.existing_servers, plan)
        except Exception as exc:
            plan = None
            content = str(exc)
        self.batch_preview.configure(state="normal")
        self.batch_preview.delete("1.0", END)
        self.batch_preview.insert("1.0", content)
        self.batch_preview.configure(state="disabled")
        self.update_batch_conflicts(plan)

    def batch_action_label(self, action: str) -> str:
        return {
            "import": self.t("batch_import"),
            "ignore": self.t("batch_ignore"),
            "overwrite": self.t("batch_overwrite"),
        }.get(action, action)

    def batch_action_from_label(self, label: str) -> str:
        if label == self.t("batch_import"):
            return "import"
        if label == self.t("batch_overwrite"):
            return "overwrite"
        return "ignore"

    def batch_item_default_action(self, item: dict) -> str:
        return "import" if item.get("status") == "NEW" else "ignore"

    def batch_item_action_choices(self, item: dict) -> list[str]:
        status = item.get("status")
        actions = ["import"] if status == "NEW" else ["ignore"]
        if item.get("can_overwrite"):
            actions.append("overwrite")
        return [self.batch_action_label(action) for action in actions]

    def compact_dict_text(self, value: dict | None) -> str:
        if not value:
            return "-"
        keys = ["name", "host_alias", "host", "user", "port", "auth", "key_file", "connection_method", "remote_path", "mountpoint"]
        lines = []
        for key in keys:
            if key in value and value.get(key) not in (None, ""):
                lines.append(f"{key}: {value.get(key)}")
        return "\n".join(lines) if lines else "-"

    def show_batch_item_details(self, item: dict, action_var: StringVar) -> None:
        text = self.t(
            "batch_detail_text",
            host=item.get("host") or "",
            status=item.get("status") or "",
            reason=item.get("reason") or "",
            action=self.batch_action_label(action_var.get()),
            server=self.compact_dict_text(item.get("server")),
            match=self.compact_dict_text(item.get("match")),
        )
        self.show_text_window(self.t("batch_detail_title"), text)

    def show_text_window(self, title: str, content: str) -> None:
        window = Toplevel(self.window)
        window.title(title)
        window.geometry("640x420")
        frame = Frame(window, padx=10, pady=10)
        frame.pack(fill=BOTH, expand=True)
        scrollbar = Scrollbar(frame)
        text = Text(frame, wrap="word", yscrollcommand=scrollbar.set)
        scrollbar.configure(command=text.yview)
        text.insert("1.0", content)
        text.configure(state="disabled")
        text.pack(side=LEFT, fill=BOTH, expand=True)
        scrollbar.pack(side=RIGHT, fill=Y)
        buttons = Frame(window, padx=10, pady=8)
        buttons.pack(fill=X)

        def copy_content() -> None:
            self.window.clipboard_clear()
            self.window.clipboard_append(content)

        Button(buttons, text=self.t("copy"), command=copy_content).pack(side=RIGHT)
        Button(buttons, text=self.t("close"), command=window.destroy).pack(side=RIGHT, padx=6)

    def update_batch_conflicts(self, plan: dict | None) -> None:
        if not hasattr(self, "batch_conflicts_body"):
            return
        for child in self.batch_conflicts_body.winfo_children():
            child.destroy()
        self.batch_conflict_actions = {}
        items = list(plan.get("items", [])) if plan else []
        if not items:
            if hasattr(self, "batch_conflicts_frame"):
                self.batch_conflicts_frame.pack_forget()
            return
        self.batch_conflicts_frame.pack(fill=X, padx=10, pady=(8, 0))
        for item in items:
            row = Frame(self.batch_conflicts_body)
            row.pack(fill=X, pady=2)
            match = item.get("match") or {}
            match_label = match.get("name") or match.get("host_alias") or match.get("id") or ""
            reason = item.get("reason") or ""
            label = f"{item['host']} - {item['status']}"
            if reason:
                label += f" - {reason}"
            if match_label:
                label += f" ({match_label})"
            Label(row, text=label, anchor="w", width=42).pack(side=LEFT, fill=X, expand=True)
            action = StringVar(value=self.batch_item_default_action(item))
            values = self.batch_item_action_choices(item)
            combo = ttk.Combobox(row, values=values, width=12, state="readonly")
            combo.set(self.batch_action_label(action.get()))

            def on_action_changed(_event=None, var=action, widget=combo):
                var.set(self.batch_action_from_label(widget.get()))

            combo.bind("<<ComboboxSelected>>", on_action_changed)
            combo.pack(side=RIGHT)
            Button(row, text=self.t("batch_details"), command=lambda current=item, var=action: self.show_batch_item_details(current, var)).pack(side=RIGHT, padx=(4, 6))
            self.batch_conflict_actions[str(item["host"])] = (item, action)
        self.update_scrollregion()

    def on_canvas_configure(self, event) -> None:
        self.canvas.itemconfigure(self.form_window, width=event.width)
        self.update_scrollregion()

    def update_scrollregion(self) -> None:
        self.canvas.update_idletasks()
        bbox = self.canvas.bbox("all")
        if not bbox:
            self.canvas.configure(scrollregion=(0, 0, self.canvas.winfo_width(), self.canvas.winfo_height()))
            return
        x1, y1, x2, y2 = bbox
        height = max(y2 - y1, self.canvas.winfo_height())
        self.canvas.configure(scrollregion=(0, 0, max(x2 - x1, self.canvas.winfo_width()), height))
        if y2 - y1 <= self.canvas.winfo_height():
            self.canvas.yview_moveto(0)

    def can_scroll_form(self) -> bool:
        bbox = self.canvas.bbox("all")
        if not bbox:
            return False
        return (bbox[3] - bbox[1]) > self.canvas.winfo_height()

    def on_mousewheel(self, event) -> None:
        if not self.can_scroll_form():
            self.canvas.yview_moveto(0)
            return "break"
        if getattr(event, "num", None) == 4:
            direction = -1
        elif getattr(event, "num", None) == 5:
            direction = 1
        else:
            delta = getattr(event, "delta", 0)
            direction = -1 if delta > 0 else 1
        self.canvas.yview_scroll(direction, "units")
        return "break"

    def bind_mousewheel_recursive(self, widget) -> None:
        if widget is getattr(self, "batch_preview", None):
            return
        widget.bind("<MouseWheel>", self.on_mousewheel)
        widget.bind("<Button-4>", self.on_mousewheel)
        widget.bind("<Button-5>", self.on_mousewheel)
        for child in widget.winfo_children():
            self.bind_mousewheel_recursive(child)

    def on_destroy(self, event) -> None:
        if event.widget is self.window:
            self.window.unbind_all("<MouseWheel>")
            self.window.unbind_all("<Button-4>")
            self.window.unbind_all("<Button-5>")

    def pick_file(self, key: str) -> None:
        path = filedialog.askopenfilename()
        if path:
            self.set_value(key, path)

    def get(self, key: str) -> str:
        entry = self.values.get(key)
        return entry.get().strip() if entry else ""

    def set_value(self, key: str, value: str) -> None:
        entry = self.values.get(key)
        if not entry:
            return
        entry.delete(0, END)
        entry.insert(0, value or "")

    def update_source_controls(self) -> None:
        source = self.source.get()
        batch = source == "ssh_config_batch"
        if batch:
            self.single_frame.pack_forget()
            self.batch_frame.pack(fill=BOTH, expand=True)
            self.save_button.configure(text=self.t("import_configs"))
        else:
            self.batch_frame.pack_forget()
            self.single_frame.pack(fill=X)
            self.save_button.configure(text=self.t("save"))
        if source == "ssh_config":
            state = "readonly"
            ssh_config_state = "disabled"
        elif source == "ssh_config_batch":
            state = "disabled"
            ssh_config_state = "disabled"
        else:
            state = "normal"
            ssh_config_state = "normal"
        self.host_combo.configure(state=state)
        for widget in (getattr(self, "write_ssh_config_check", None), getattr(self, "copy_key_check", None)):
            if widget:
                try:
                    widget.configure(state=ssh_config_state)
                except Exception:
                    pass
        self.update_connection_method_controls()
        self.update_scrollregion()

    def set_required_star(self, key: str, visible: bool) -> None:
        star = self.required_stars.get(key)
        if not star:
            return
        if visible:
            if not star.winfo_manager():
                star.pack(side=LEFT, padx=(2, 0))
        else:
            star.pack_forget()

    def update_required_stars(self) -> None:
        single_config = self.source.get() != "ssh_config_batch"
        native_rclone = self.connection_method.get() != "openssh"
        self.set_required_star("key_file", single_config and native_rclone and self.auth.get() == "key")
        self.set_required_star("password", single_config and native_rclone and self.auth.get() == "password")

    def update_connection_method_controls(self) -> None:
        if not hasattr(self, "connection_help"):
            return
        openssh = self.connection_method.get() == "openssh" and self.source.get() != "ssh_config_batch"
        if openssh:
            self.auth.set("key")
            self.connection_help.pack(fill=X, padx=24, pady=(0, 4))
        else:
            self.connection_help.pack_forget()

        secret_state = "disabled" if openssh else "normal"
        for key in ("key_passphrase", "password"):
            widget = self.values.get(key)
            if widget:
                try:
                    widget.configure(state=secret_state)
                except Exception:
                    pass
        auth_state = "disabled" if openssh else "normal"
        for button in getattr(self, "auth_buttons", []):
            try:
                button.configure(state=auth_state)
            except Exception:
                pass
        copy_state = "normal"
        if self.source.get() in {"ssh_config", "ssh_config_batch"} or self.auth.get() != "key":
            copy_state = "disabled"
        if getattr(self, "copy_key_check", None):
            try:
                self.copy_key_check.configure(state=copy_state)
            except Exception:
                pass
        self.update_required_stars()

    def on_source_changed(self) -> None:
        self.update_source_controls()
        if self.source.get() == "ssh_config" and self.get("host_alias"):
            self.apply_ssh_defaults(self.get("host_alias"))
        elif self.source.get() == "sai_cluster":
            self.apply_sai_defaults()
        elif self.source.get() == "manual" and not self.existing:
            self.apply_manual_defaults()

    def on_ssh_host_selected(self, _event=None) -> None:
        self.apply_ssh_defaults(self.get("host_alias"))

    def apply_ssh_defaults(self, host_alias: str) -> None:
        try:
            defaults = ssh_config_defaults(host_alias)
        except Exception as exc:
            messagebox.showerror(APP_TITLE, str(exc))
            return
        for key in ["name", "host", "user", "port", "key_file"]:
            self.set_value(key, defaults.get(key, ""))
        self.auth.set("key" if defaults.get("key_file") else self.auth.get())
        self.update_connection_method_controls()

    def apply_sai_defaults(self) -> None:
        defaults = sai_cluster_defaults("")
        for key in ["name", "host_alias", "host", "user", "port", "key_file"]:
            self.set_value(key, defaults.get(key, ""))
        self.auth.set(defaults["auth"])
        self.connection_method.set(defaults["connection_method"])
        self.write_ssh_config.set(False)
        self.copy_key_to_ssh.set(False)
        base, suffix = split_remote_path(defaults["remote_path"])
        self.set_value("remote_base", base)
        self.set_value("remote_suffix", suffix)
        self.set_value("mountpoint", mountpoint_value_to_choice(defaults["mountpoint"], self.lang))
        self.last_sai_profile_name = defaults["name"]
        self.update_connection_method_controls()

    def apply_manual_defaults(self) -> None:
        for key in ["name", "host_alias", "host", "user", "port", "key_file", "key_passphrase", "password"]:
            self.set_value(key, "")
        self.auth.set("key")
        self.connection_method.set("native")
        self.write_ssh_config.set(False)
        self.copy_key_to_ssh.set(False)
        base, suffix = split_remote_path("")
        self.set_value("remote_base", base)
        self.set_value("remote_suffix", suffix)
        self.set_value("mountpoint", mountpoint_value_to_choice("", self.lang))
        self.update_connection_method_controls()

    def on_sai_user_changed(self, _event=None) -> None:
        if self.source.get() != "sai_cluster":
            return
        name = sai_profile_name(self.get("user"))
        old_name = self.last_sai_profile_name
        for key in ("name", "host_alias"):
            current = self.get(key)
            if current in {"", "SAI", old_name} or current.startswith("SAI-"):
                self.set_value(key, name)
        self.last_sai_profile_name = name

    def show_validation_error(self, message: str) -> None:
        messagebox.showerror(APP_TITLE, message)

    def validation_message(self, exc: Exception) -> str:
        text = str(exc)
        if text.startswith("SSH Host is required"):
            return self.t("ssh_host_required")
        if text.startswith("SSH Host"):
            return self.t("ssh_host_invalid")
        if text.startswith("Port"):
            return self.t("port_invalid")
        if text.startswith("Select a private key file"):
            return self.t("key_file_required")
        if text.startswith("Select the private key file"):
            return self.t("private_key_required")
        if text.startswith("Key file not found:"):
            return self.t("key_file_not_found", path=text.split(":", 1)[1].strip())
        return text

    def duplicate_target_name(self, result: dict) -> str:
        current_id = str(self.existing.get("id") or "")
        current_target = exact_connection_fingerprint(result)
        for server in self.existing_servers:
            if current_id and str(server.get("id") or "") == current_id:
                continue
            if exact_connection_fingerprint(server) == current_target:
                return str(server.get("name") or server.get("host_alias") or server.get("id") or "")
        return ""

    def save(self) -> None:
        source = self.source.get()
        if source == "ssh_config_batch":
            plan = ssh_config_batch_plan(self.batch_config_path.get(), self.existing_servers)
            action_items = self.batch_conflict_actions
            if not action_items:
                self.update_batch_conflicts(plan)
                action_items = self.batch_conflict_actions
            servers = []
            overwrite_count = 0
            ignored_count = 0
            for item in plan.get("items", []):
                action_entry = action_items.get(str(item.get("host") or ""))
                action = action_entry[1].get() if action_entry else self.batch_item_default_action(item)
                if action == "ignore":
                    ignored_count += 1
                    continue
                if action == "import":
                    if item.get("status") == "NEW" and item.get("server"):
                        servers.append(dict(item["server"]))
                    else:
                        ignored_count += 1
                    continue
                if action != "overwrite":
                    ignored_count += 1
                    continue
                match = item.get("match") or {}
                if not match.get("id"):
                    ignored_count += 1
                    continue
                server = dict(item.get("server") or {})
                server["__batch_action"] = "overwrite"
                server["__batch_target_id"] = str(match["id"])
                servers.append(server)
                overwrite_count += 1
            skipped = [*plan["skipped"], *plan["errors"]]
            if not servers:
                message = self.t("no_importable_hosts")
                if skipped:
                    message += "\n\n" + "\n".join(f"{item['host']}: {item['status']} {item.get('reason', '')}".strip() for item in skipped)
                messagebox.showerror(APP_TITLE, message)
                return
            if ignored_count:
                messagebox.showinfo(APP_TITLE, self.t("batch_import_notice", new_count=len(servers), skip_count=ignored_count))
            self.result = servers
            self.window.destroy()
            return

        name = self.get("name")
        if not name:
            self.show_validation_error(self.t("field_required", field=self.t("name")))
            return
        host = self.get("host")
        user = self.get("user")
        if not host:
            self.show_validation_error(self.t("field_required", field=self.t("ip_host")))
            return
        if not user:
            self.show_validation_error(self.t("field_required", field=self.t("user")))
            return
        try:
            port = normalized_port(self.get("port"))
        except ValueError:
            self.show_validation_error(self.t("port_invalid"))
            return
        if source == "sai_cluster":
            sai_name = sai_profile_name(user)
            if name in {"", "SAI"} or str(name).startswith("SAI-"):
                name = sai_name

        server_id = self.existing.get("id") or "".join(ch if ch.isalnum() or ch in "._-" else "_" for ch in name)
        mountpoint = mountpoint_choice_to_value(self.get("mountpoint"), self.lang)
        mountpoint_error = validate_mountpoint_for_save(mountpoint)
        if mountpoint_error:
            messagebox.showerror(APP_TITLE, self.t("invalid_mountpoint", reason=mountpoint_error))
            return
        ssh_config_managed = bool(self.write_ssh_config.get() and source not in {"ssh_config", "ssh_config_batch"})
        host_alias = self.get("host_alias")
        if source == "sai_cluster" and user and (not host_alias or host_alias == "SAI" or str(host_alias).startswith("SAI-")):
            host_alias = sai_profile_name(user)
        if ssh_config_managed and not host_alias:
            host_alias = ssh_safe_name(name)

        result = {
            "id": server_id,
            "name": name,
            "mode": "manual",
            "source": source,
            "host_alias": host_alias if source == "ssh_config" or ssh_config_managed else "",
            "host": host,
            "user": user,
            "port": port,
            "auth": self.auth.get(),
            "key_file": self.get("key_file"),
            "connection_method": self.connection_method.get() or "native",
            "remote_path": compose_remote_path(self.get("remote_base"), self.get("remote_suffix")),
            "mountpoint": mountpoint,
            "cache_mode": self.existing.get("cache_mode", ""),
            "ssh_config_managed": ssh_config_managed,
            "copy_key_to_ssh_dir": bool(self.copy_key_to_ssh.get() and ssh_config_managed and self.auth.get() == "key"),
        }

        duplicate_name = self.duplicate_target_name(result)
        if duplicate_name:
            self.show_validation_error(self.t("duplicate_target", name=duplicate_name))
            return

        if result["connection_method"] != "openssh" and result["auth"] == "key":
            try:
                result["key_file"] = validate_private_key_path(result.get("key_file", ""))
            except Exception as exc:
                self.show_validation_error(self.validation_message(exc))
                return

        if result["ssh_config_managed"]:
            try:
                validate_managed_ssh_config_server(result)
            except Exception as exc:
                self.show_validation_error(self.validation_message(exc))
                return
        if result["copy_key_to_ssh_dir"] and not result.get("key_file"):
            self.show_validation_error(self.t("key_file_required"))
            return
        if result["copy_key_to_ssh_dir"] and result.get("key_file"):
            try:
                result["key_file"] = copy_key_to_user_ssh(result["key_file"], result["host_alias"] or result["name"])
            except Exception as exc:
                messagebox.showerror(APP_TITLE, self.validation_message(exc))
                return

        if self.connection_method.get() == "openssh":
            result["auth"] = "key"
        elif self.auth.get() == "password":
            password = self.get("password")
            if not password and self.existing.get("password_obscured") and same_password_target(self.existing, result):
                result["password_obscured"] = self.existing["password_obscured"]
            elif not password:
                messagebox.showerror(APP_TITLE, self.t("password_required"))
                return
            else:
                try:
                    result["password_obscured"] = obscure_password(self.rclone, password)
                except Exception as exc:
                    messagebox.showerror(APP_TITLE, str(exc))
                    return
        elif self.auth.get() == "key":
            key_passphrase = self.get("key_passphrase")
            if key_passphrase:
                try:
                    result["key_pass_obscured"] = obscure_password(self.rclone, key_passphrase)
                except Exception as exc:
                    messagebox.showerror(APP_TITLE, str(exc))
                    return
            elif self.existing.get("key_pass_obscured") and same_key_passphrase_target(self.existing, result):
                result["key_pass_obscured"] = self.existing["key_pass_obscured"]
        if result["ssh_config_managed"]:
            try:
                managed_config = write_managed_ssh_config(result)
                result["managed_ssh_config_path"] = str(managed_config)
            except Exception as exc:
                messagebox.showerror(APP_TITLE, str(exc))
                return
        self.result = result
        self.window.destroy()


def ignore_macos_launchservice_args(argv: list[str]) -> list[str]:
    return [arg for arg in argv if not arg.startswith("-psn_")]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", action="version", version=f"{APP_TITLE} {VERSION}")
    parser.add_argument("--install-help", action="store_true", help="Print manual dependency install commands and exit.")
    parser.add_argument("--licenses", action="store_true", help="Print bundled third-party notices and licenses and exit.")
    parser.add_argument("--check-update", action="store_true", help="Check the latest GitHub release and exit.")
    parser.add_argument("--mount-id")
    args = parser.parse_args(ignore_macos_launchservice_args(sys.argv[1:]))
    if args.install_help:
        print(manual_install_text())
        return 0
    if args.licenses:
        print(THIRD_PARTY_NOTICES)
        return 0
    if args.check_update:
        try:
            print(format_update_info(check_for_updates(VERSION)))
            return 0
        except Exception as exc:
            print(f"Update check failed: {exc}", file=sys.stderr)
            return 1
    if args.mount_id:
        return headless_mount(args.mount_id)
    root = Tk()
    App(root)
    root.mainloop()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
