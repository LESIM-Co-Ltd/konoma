#!/bin/bash
# usage: run_job.sh gen_pivot.py      (run from anywhere; WORK=<scratch dir> optional)
# Runs one generator inside a one-shot headless soffice with a throw-away profile.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
WORK="${WORK:-$HERE/../../../../NoCode/.cache/office-complex}"
mkdir -p "$WORK"
WORK="$(cd "$WORK" && pwd)"
PROFILE="$WORK/lo-profile"
if [ ! -d "$PROFILE/user/Scripts/python" ]; then
  # first run: let soffice create the profile, then install the runner and force an en-US locale
  timeout 120 /Applications/LibreOffice.app/Contents/MacOS/soffice --headless --invisible --norestore --nolockcheck \
    -env:UserInstallation=file://$PROFILE --terminate_after_init >/dev/null 2>&1
  mkdir -p "$PROFILE/user/Scripts/python" "$PROFILE/user/basic/Standard"
  cp "$HERE/lo_runner/runner.py" "$PROFILE/user/Scripts/python/runner.py"
  cp "$HERE/lo_runner/Module1.xba" "$PROFILE/user/basic/Standard/Module1.xba"
  python3 - "$PROFILE/user/registrymodifications.xcu" <<'PY'
import sys
p = sys.argv[1]
s = open(p).read()
add = ('<item oor:path="/org.openoffice.Setup/L10N"><prop oor:name="ooSetupSystemLocale" oor:op="fuse"><value>en-US</value></prop></item>\n'
       '<item oor:path="/org.openoffice.Setup/L10N"><prop oor:name="ooLocale" oor:op="fuse"><value>en-US</value></prop></item>\n')
if "ooSetupSystemLocale" not in s:
    s = s.replace("</oor:items>", add + "</oor:items>")
open(p, "w").write(s)
PY
fi
export GEN_DIR="$HERE" JOB_FILE="$WORK/job.txt" LOG_FILE="$WORK/job.log" WORK_DIR="$WORK"
echo "$1" > "$JOB_FILE"
rm -f "$LOG_FILE"
timeout 500 /Applications/LibreOffice.app/Contents/MacOS/soffice --headless --invisible --norestore --nolockcheck \
  -env:UserInstallation=file://$PROFILE "macro:///Standard.Module1.RunJob" 2>&1 | grep -v "Task policy"
cat "$LOG_FILE"
