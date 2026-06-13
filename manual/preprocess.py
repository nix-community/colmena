# Markdown preprocessor
#
# 1. Removes marked blocks from markdown files
#    (unstable-only text from stable versions and vice versa)
# 2. Substitutes @version@ in text with the full version
# 3. Substitutes @apiVersion@ in text with major.minor
#
# Environment:
# - COLMENA_VERSION=${fullVersion}
# - COLMENA_UNSTABLE="1" or unset

import json
import os
import re
import sys


def eprint(*args, **kwargs):
    print(*args, file=sys.stderr, **kwargs)


def process(book, version, unstable):
    marker_to_remove = "STABLE" if unstable else "UNSTABLE"

    api_version = re.match(r"^[0-9]+\.[0-9]+(?=$|\.[0-9]+(.*)?$)", version)
    if api_version:
        api_version = api_version.group()
    else:
        api_version = version

    version_debug = f"{version} " \
        f"(apiVersion={api_version}, unstable={str(unstable)})"

    def replace_version_markers(s):
        s = s.replace("@version@", version)
        s = s.replace("@apiVersion@", api_version)
        return s

    def process_item(item):
        chapter = item["Chapter"]

        for sub_item in chapter["sub_items"]:
            process_item(sub_item)

        regex = r".*\b{marker}_BEGIN\b(.|\n|\r)*?\b{marker}_END\b.*" \
            .format(marker=marker_to_remove)

        chapter["content"] = \
            f"<!-- Generated from version {version_debug} -->\n" \
            + replace_version_markers(re.sub(regex, "", chapter["content"]))

    eprint(f"Version is {version_debug}")

    for section in book['items']:
        process_item(section)


if __name__ == "__main__":
    if len(sys.argv) > 1:
        if sys.argv[1] == "supports":
            sys.exit(0)

    version = os.environ.get("COLMENA_VERSION", "unstable")
    unstable = bool(os.environ.get("COLMENA_UNSTABLE", ""))

    if version in ["", "unstable"]:
        unstable = True

    context, book = json.load(sys.stdin)

    process(book, version, unstable)

    print(json.dumps(book))
