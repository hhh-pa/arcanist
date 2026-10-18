---- MODULE FirstWins ----
(*
 * T1176 MC: the OnceLock first-wins contract our tests rely on
 * (T1153 CORE-19 command cell; the merged core19/call-cond regression
 * test pins it). Writers race a single set; readers observe. TLC
 * proves, over every interleaving: every observed value equals the
 * cell (coherence), and once set the cell agrees with its winner
 * forever (stability / set-once).
 *
 * Each writer writes its own identity (mirrors the real "help" vs
 * "call-conduit" race): a torn or flipped outcome would show up as a
 * second distinct observed value.
 *)

EXTENDS Naturals, TLC

CONSTANTS Writers, Readers

VARIABLES cell, winner, seen, wpc, rpc

vars == <<cell, winner, seen, wpc, rpc>>

TypeOK ==
    /\ cell \in Writers \cup {"none"}
    /\ winner \in Writers \cup {"none"}
    /\ seen \in [Readers -> Writers \cup {"none"}]
    /\ wpc \in [Writers -> {"set", "done"}]
    /\ rpc \in [Readers -> {"read", "done"}]

Init ==
    /\ cell = "none"
    /\ winner = "none"
    /\ seen = [r \in Readers |-> "none"]
    /\ wpc = [w \in Writers |-> "set"]
    /\ rpc = [r \in Readers |-> "read"]

Set(w) ==
    /\ wpc[w] = "set"
    /\ IF cell = "none"
       THEN /\ cell' = w
            /\ winner' = w
       ELSE UNCHANGED <<cell, winner>>
    /\ wpc' = [wpc EXCEPT ![w] = "done"]
    /\ UNCHANGED <<seen, rpc>>

Read(r) ==
    /\ rpc[r] = "read"
    /\ seen' = [seen EXCEPT ![r] = cell]
    /\ rpc' = [rpc EXCEPT ![r] = "done"]
    /\ UNCHANGED <<cell, winner, wpc>>

AllDone ==
    (\A w \in Writers : wpc[w] = "done") /\
    (\A r \in Readers : rpc[r] = "done")

\* Successful termination stutters forever, so TLC only reports *genuine*
\* deadlocks, never mere completion.
Next == (\E w \in Writers : Set(w)) \/ (\E r \in Readers : Read(r))
         \/ (AllDone /\ UNCHANGED vars)

Spec == Init /\ [][Next]_vars /\ WF_vars(Next)

Coherence ==
    \A r1, r2 \in Readers :
        (seen[r1] # "none" /\ seen[r2] # "none") => seen[r1] = seen[r2]

Consistent ==
    (cell # "none") => (winner # "none" /\ cell = winner)

Liveness == <>AllDone

====
