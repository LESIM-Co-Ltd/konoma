"""Runs one generator script *inside* a headless soffice (macro entry point).

Why not a plain `python` + UNO socket: on the machine this was built on, LibreOffice's bundled
Python binary is killed on launch (SIGKILL), so the script runs in soffice's own embedded Python.
Paths come from the environment set by run_job.sh (GEN_DIR, JOB_FILE, LOG_FILE, WORK_DIR).
"""
import os
import sys
import traceback


def run(*args):
    gen = os.environ["GEN_DIR"]
    script = open(os.environ["JOB_FILE"]).read().strip()
    out = open(os.environ["LOG_FILE"], "w", buffering=1)
    old = sys.stdout, sys.stderr
    sys.stdout = sys.stderr = out
    os.environ["LO_INPROC"] = "1"
    try:
        sys.path.insert(0, gen)
        os.chdir(gen)
        # exec (not runpy): runpy leaves pyuno's runtime "not initialized" for the script
        code = compile(open(os.path.join(gen, script)).read(), script, "exec")
        exec(code, {"__name__": "__main__", "XSCRIPTCONTEXT": XSCRIPTCONTEXT})
        print("JOB-OK")
    except BaseException:
        traceback.print_exc()
        print("JOB-FAILED")
    finally:
        sys.stdout, sys.stderr = old
        out.close()
        try:
            # a one-shot soffice stays alive while a document is open: end it
            XSCRIPTCONTEXT.getDesktop().terminate()
        except Exception:
            pass
