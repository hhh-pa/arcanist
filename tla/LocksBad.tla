---- MODULE LocksBad ----
(*
 * T1176 MC: the integration-test guard discipline, modeled and proven
 * with TLC (TLA+). `C` = CWD_LOCK (common::CwdGuard), `E` = ENV_LOCK
 * (common::HomeGuard). Threads acquire the locks in `Need(t)` order,
 * hold them for the critical section, then release one at a time.
 *
 * NEGATIVE CONTROL for Locks.tla: identical model, but t1 takes E
 * then C while t3 takes C then E (two dual-lock threads with inverted
 * orders). A single dual-lock thread can never deadlock against
 * single-lock peers, so the inversion needs both sides to form the
 * embrace. TLC MUST report a genuine deadlock here; if it does not,
 * the Locks.tla proof is vacuous and must not be trusted.
 *)

EXTENDS Naturals, Sequences, TLC

Threads == {"t1", "t2", "t3"}

\* INVERTED orders (negative control): t1 takes E then C while t3 takes
\* C then E. t1 holding E and waiting on C while t3 holds C and waits on
\* E is the classic deadly embrace TLC must find.
Need(t) == IF t = "t1" THEN <<"E", "C">>
           ELSE IF t = "t2" THEN <<"E">>
           ELSE <<"C", "E">>

VARIABLES holder, pc, held, next

vars == <<holder, pc, held, next>>

TypeOK ==
    /\ holder \in [{"C", "E"} -> Threads \cup {"none"}]
    /\ pc \in [Threads -> {"acquire", "release", "done"}]
    /\ held \in [Threads -> SUBSET {"C", "E"}]
    /\ next \in [Threads -> Nat]

Init ==
    /\ holder = [l \in {"C", "E"} |-> "none"]
    /\ pc = [t \in Threads |-> "acquire"]
    /\ held = [t \in Threads |-> {}]
    /\ next = [t \in Threads |-> 1]

Acquire(t) ==
    /\ pc[t] = "acquire"
    /\ next[t] <= Len(Need(t))
    /\ LET l == Need(t)[next[t]]
       IN /\ holder[l] = "none"
          /\ holder' = [holder EXCEPT ![l] = t]
          /\ held' = [held EXCEPT ![t] = @ \cup {l}]
          /\ next' = [next EXCEPT ![t] = @ + 1]
          /\ pc' = [pc EXCEPT ![t] =
                IF next[t] + 1 > Len(Need(t)) THEN "release" ELSE "acquire"]

Release(t) ==
    /\ pc[t] = "release"
    /\ held[t] # {}
    /\ \E l \in held[t] :
        /\ holder' = [holder EXCEPT ![l] = "none"]
        /\ held' = [held EXCEPT ![t] = @ \ {l}]
        /\ pc' = [pc EXCEPT ![t] =
                IF held[t] \ {l} = {} THEN "done" ELSE "release"]
    /\ UNCHANGED next

\* Successful termination stutters forever, so TLC only reports *genuine*
\* deadlocks (threads stuck mid-acquire), never mere completion.
Terminating == (\A t \in Threads : pc[t] = "done") /\ UNCHANGED vars

Next == (\E t \in Threads : Acquire(t) \/ Release(t)) \/ Terminating

Spec == Init /\ [][Next]_vars /\ WF_vars(Next)

AllDone == \A t \in Threads : pc[t] = "done"

Liveness == <>AllDone

====
