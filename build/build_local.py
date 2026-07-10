from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
from pathlib import Path


def subset_text(root: Path) -> str:
    sys.path.insert(0, str(root / "src"))
    from ssh_mountmate.gui import (
        BUFFER_SIZE_CHOICES,
        CACHE_AGE_CHOICES,
        CACHE_SIZE_CHOICES,
        DIR_CACHE_TIME_CHOICES,
        LANGUAGE_CHOICES,
        MIN_FREE_CHOICES,
        TEXT,
        WRITE_BACK_CHOICES,
    )

    values: list[str] = []
    for language in TEXT.values():
        values.extend(str(value) for value in language.values())
    values.extend(LANGUAGE_CHOICES.values())
    values.extend(CACHE_SIZE_CHOICES)
    values.extend(CACHE_AGE_CHOICES)
    values.extend(MIN_FREE_CHOICES)
    values.extend(WRITE_BACK_CHOICES)
    values.extend(DIR_CACHE_TIME_CHOICES)
    values.extend(BUFFER_SIZE_CHOICES)
    values.extend(
        [
            "".join(chr(codepoint) for codepoint in range(0x20, 0x7F)),
            "。，、；：？！“”‘’（）【】《》—…·￥",
            "🛡📂✎🗑■▶",
        ]
    )
    return "".join(sorted(set("".join(values))))


def prepare_assets(root: Path) -> Path:
    source_assets = root / "src" / "ssh_mountmate" / "assets"
    source_fonts = source_assets / "fonts"
    generated_assets = root / "build" / "generated-assets" / "assets"
    generated_fonts = generated_assets / "fonts"
    if generated_assets.exists():
        shutil.rmtree(generated_assets)
    generated_fonts.mkdir(parents=True, exist_ok=True)

    shutil.copy2(source_fonts / "LICENSE-Noto-CJK.txt", generated_fonts / "LICENSE-Noto-CJK.txt")
    text_file = generated_fonts / "subset-chars.txt"
    text_file.write_text(subset_text(root), encoding="utf-8")

    source_font = source_fonts / "NotoSansCJKsc-Regular.otf"
    target_font = generated_fonts / "NotoSansCJKsc-Regular.otf"
    subprocess.check_call(
        [
            sys.executable,
            "-m",
            "fontTools.subset",
            str(source_font),
            f"--output-file={target_font}",
            f"--text-file={text_file}",
            "--layout-features=*",
            "--name-IDs=*",
            "--name-legacy",
            "--name-languages=*",
            "--notdef-glyph",
            "--notdef-outline",
            "--recommended-glyphs",
        ]
    )
    text_file.unlink(missing_ok=True)
    print(f"Generated subset font: {target_font} ({target_font.stat().st_size:,} bytes)")
    return generated_assets


def prepare_rclone_binary(root: Path) -> Path:
    sys.path.insert(0, str(root / "src"))
    from ssh_mountmate.rclone import install_rclone_to, rclone_version

    generated_bin = root / "build" / "generated-bin" / "bin"
    if generated_bin.exists():
        shutil.rmtree(generated_bin)
    generated_bin.mkdir(parents=True, exist_ok=True)

    rclone = install_rclone_to(generated_bin)
    print(f"Bundled rclone: {rclone} ({rclone_version(str(rclone))})")
    return rclone


def build_variant(root: Path, assets: Path, rclone: Path, mode: str) -> int:
    dist = root / "dist" / mode
    work = root / "build" / f"pyinstaller-work-{mode}"
    data_separator = ";" if sys.platform.startswith("win") else ":"
    cmd = [
        sys.executable,
        "-m",
        "PyInstaller",
        "--name",
        "SSHMountMate",
        "--windowed",
        "--onefile" if mode == "onefile" else "--onedir",
        "--distpath",
        str(dist),
        "--workpath",
        str(work),
        "--specpath",
        str(root / "build"),
        "--add-data",
        f"{assets}{data_separator}assets",
        "--add-binary",
        f"{rclone}{data_separator}bin",
        str(root / "launcher.py"),
    ]
    return subprocess.call(cmd)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mode", choices=("onefile", "onedir", "both"), default="both")
    args = parser.parse_args(argv)
    root = Path(__file__).resolve().parents[1]
    dist = root / "dist"
    if dist.exists():
        shutil.rmtree(dist)
    assets = prepare_assets(root)
    rclone = prepare_rclone_binary(root)
    modes = ("onefile", "onedir") if args.mode == "both" else (args.mode,)
    for mode in modes:
        result = build_variant(root, assets, rclone, mode)
        if result:
            return result
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
