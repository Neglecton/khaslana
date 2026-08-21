# 生成更新清单（由 release.yml 的 Package 步骤调用，工作目录 dist/）。
#
# 输入经环境变量传入（避免 shell 引号/多行转义问题）：
#   MANIFEST_VERSION / MANIFEST_CHANNEL(stable|beta) / MANIFEST_TAG /
#   MANIFEST_ZIP / MANIFEST_SHA / MANIFEST_SIZE / MANIFEST_NOTES
#
# 输出（schema=1，与旧客户端严格兼容——旧字段一个不少、格式不变）：
#   正式版：khaslana-update.json + khaslana-update-beta.json（同内容）
#   测试版：仅 khaslana-update-beta.json
import datetime
import json
import os
import sys

version = os.environ["MANIFEST_VERSION"]
channel = os.environ["MANIFEST_CHANNEL"]
tag = os.environ["MANIFEST_TAG"]
zip_name = os.environ["MANIFEST_ZIP"]
sha = os.environ["MANIFEST_SHA"]
size = os.environ["MANIFEST_SIZE"].strip()
notes = os.environ.get("MANIFEST_NOTES", "").strip()
if not notes:
    notes = f"Release {tag}"

manifest = {
    "schema": 1,
    "channel": channel,
    "version": version,
    "published_at": datetime.datetime.now(datetime.timezone.utc).strftime(
        "%Y-%m-%dT%H:%M:%SZ"
    ),
    "notes": notes,
    "platforms": {
        "windows-x86_64": {
            "archive_url": (
                "https://cnb.cool/suhoan/khaslana-release/-/git/raw/master/"
                f"releases/v{version}/{zip_name}.zip"
            ),
            "fallback_archive_url": (
                "https://github.com/FuturePrayer/khaslana/releases/download/"
                f"{tag}/{zip_name}.zip"
            ),
            "sha256": sha,
            "size": int(size),
        }
    },
}

beta_path = "khaslana-update-beta.json"
with open(beta_path, "w", encoding="utf-8", newline="\n") as f:
    json.dump(manifest, f, ensure_ascii=False, indent=2)
    f.write("\n")

if channel == "stable":
    with open("khaslana-update.json", "w", encoding="utf-8", newline="\n") as f:
        json.dump(manifest, f, ensure_ascii=False, indent=2)
        f.write("\n")
    print(f"wrote khaslana-update.json + {beta_path} for stable {version}", file=sys.stderr)
else:
    print(f"wrote {beta_path} only for beta {version}", file=sys.stderr)
