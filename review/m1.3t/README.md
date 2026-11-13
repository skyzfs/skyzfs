# M1.3t (the log store removed): what to judge when it opens

Not opened yet. Passed on by the lead on 2026-11-13 as the owner's
ruling for this review:

- Only tests of the log store's internals may go. The storage
  contract's tests stay.
- Coding owes a table of every deleted test: which kind it was, and for
  a contract or pump/lifecycle property the in-place test that now
  holds the same property (ported where none existed).
- Judge that table against the code. A deleted test whose property is
  held nowhere any more is a blocking finding.

How to check it: list the tests at the base and at the head (`cargo
test -- --list` for each crate, plain and with the features), take the
difference, and for every deleted name find the row of the table and
read the test it names; plant the property's breach in the in-place
store and see that test fail.
