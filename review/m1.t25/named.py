import sys, time
open("/proc/self/comm", "w").write(sys.argv[1])
if sys.argv[2] == "run":
    end = time.time() + float(sys.argv[3])
    while time.time() < end:
        pass
else:
    time.sleep(float(sys.argv[3]))
