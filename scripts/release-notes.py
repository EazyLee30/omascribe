#!/usr/bin/env python3
"""Put the five user downloads first; keep updater plumbing out of the download instructions."""
import argparse,pathlib
ROOT=pathlib.Path(__file__).resolve().parents[1]
def body(version):
    base=f'https://github.com/EazyLee30/omascribe/releases/download/v{version}/OmaScribe_{version}_'
    rows=[('macOS · Apple 芯片（M 系列）','macOS-AppleSilicon.dmg','DMG'),('macOS · Intel','macOS-Intel.dmg','DMG'),('Windows · Intel / AMD 64 位','Windows-x64.exe','安装程序 / Installer'),('Linux · 通用 64 位','Linux-x64.AppImage','AppImage'),('Linux · Debian / Ubuntu','Linux-x64.deb','DEB')]
    table='\n'.join(f'| {label} | [{kind}]({base}{name}) |' for label,name,kind in rows)
    return f'''# OmaScribe {version}

**选择下表与你电脑对应的一项即可。** / Choose one installer for your computer.

| 你的电脑 / Computer | 下载安装包 / Download |
| --- | --- |
{table}

Mac 芯片类型：苹果菜单 → 关于本机。所有安装包包含默认离线模型。

[官网选择下载 / Website downloads](https://omascribe.eazylee.si/download/)

<details>
<summary>其他文件是什么？ / About other assets</summary>

`.app.tar.gz`、`latest.json` 和 `latest-runtime.json` 由客户端更新器使用，无需手动下载。`SHA256SUMS.txt` 用于校验下载文件。签名和运行验证报告保存在本次 GitHub Actions 构建记录中。

</details>

---

{(ROOT/f'docs/releases/v{version}.md').read_text().removeprefix(f'# OmaScribe {version}').lstrip()}
'''
if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--version',required=True);p.add_argument('--output',required=True,type=pathlib.Path);a=p.parse_args();a.output.write_text(body(a.version),encoding='utf-8')
