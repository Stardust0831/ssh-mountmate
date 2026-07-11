from __future__ import annotations

import os
import subprocess
import sys


SHELL_VERBS = {
    r"Directory\Background\shell\SSHMountMate.Refresh": ("Refresh with SSH MountMate", "--refresh-path", "%V"),
    r"Directory\shell\SSHMountMate.Refresh": ("Refresh with SSH MountMate", "--refresh-path", "%1"),
    r"Drive\shell\SSHMountMate.Refresh": ("Refresh with SSH MountMate", "--refresh-path", "%1"),
    r"Directory\Background\shell\SSHMountMate.Transfers": ("Open SSH MountMate transfers", "--show-transfers", ""),
    r"Directory\shell\SSHMountMate.Transfers": ("Open SSH MountMate transfers", "--show-transfers", ""),
    r"Drive\shell\SSHMountMate.Transfers": ("Open SSH MountMate transfers", "--show-transfers", ""),
}


def application_command() -> list[str]:
    if getattr(sys, "frozen", False):
        return [sys.executable]
    return [sys.executable, "-m", "ssh_mountmate"]


def windows_command_line(*args: str) -> str:
    values = [arg for arg in args if arg]
    placeholder = values[-1] if values and values[-1].startswith("%") else ""
    command_args = values[:-1] if placeholder else values
    command = subprocess.list2cmdline([*application_command(), *command_args])
    return f'{command} "{placeholder}\\."' if placeholder else command


def register_windows_context_menu() -> None:
    if os.name != "nt":
        raise RuntimeError("Explorer context-menu registration is only available on Windows.")
    import winreg

    root = winreg.HKEY_CURRENT_USER
    for relative, (label, action, placeholder) in SHELL_VERBS.items():
        key_path = rf"Software\Classes\{relative}"
        with winreg.CreateKey(root, key_path) as key:
            winreg.SetValueEx(key, "", 0, winreg.REG_SZ, label)
            winreg.SetValueEx(key, "Icon", 0, winreg.REG_SZ, application_command()[0])
        with winreg.CreateKey(root, key_path + r"\command") as command_key:
            winreg.SetValueEx(command_key, "", 0, winreg.REG_SZ, windows_command_line(action, placeholder))


def delete_registry_tree(root, path: str) -> None:
    import winreg

    try:
        with winreg.OpenKey(root, path, 0, winreg.KEY_READ | winreg.KEY_WRITE) as key:
            children: list[str] = []
            index = 0
            while True:
                try:
                    children.append(winreg.EnumKey(key, index))
                    index += 1
                except OSError:
                    break
        for child in children:
            delete_registry_tree(root, path + "\\" + child)
        winreg.DeleteKey(root, path)
    except FileNotFoundError:
        return


def unregister_windows_context_menu() -> None:
    if os.name != "nt":
        raise RuntimeError("Explorer context-menu registration is only available on Windows.")
    import winreg

    for relative in SHELL_VERBS:
        delete_registry_tree(winreg.HKEY_CURRENT_USER, rf"Software\Classes\{relative}")
