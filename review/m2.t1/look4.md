## Look at a8349da (#94): the nit is settled

`record()` resets `WORST`, so each recording's subshell now exits with that recording's own status.

I ran the loop and `record()` from the script verbatim, with stub recordings, in both modes:
- **Default mode:** results as before.
  - A failed command or pipeline stops the recording and counts 2, with one `rig error` line.
  - A verdict of 1 stays 1.
  - The next recording still runs.
- **`CUT_CHECK=none`:** a good recording after a rig error now has only its `recorded` line, where before it also got a false `rig error`.

Nothing of mine is open on #94. What is left is the cut rig's table on the final head, which goes into the description.
