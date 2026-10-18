---- MODULE Locks ----
(*
 * T1176 MC: the integration-test guard discipline, modeled and proven
 * with TLC (TLA+). `C` = CWD_LOCK (common::CwdGuard), `E` = ENV_LOCK
 * (common::HomeGuard). Threads acquire the locks in `Need(t)` order,
 * hold them for the critical section, then release one at a time.
 *
 * Reality modeled: HomeGuard takes E; CwdGuard takes C; land_workflow's
 * contract test takes C then E; no test takes E then C. TLC proves every
 * thread finishes on every fair schedule (deadlock-freedom). The
 * negative control is LocksBad.tla: two dual-lock threads with inverted
 * orders (EC vs CE), where TLC MUST find the deadly embrace.
 *)

EXTENDS Naturals, Sequences, TLC

Threads == {"t1", "t2", "t3"}

\* Per-thread lock-acquisition order. Canonical production discipline: t1
\* takes C before E (CwdGuard-then-HomeGuard), t2 takes E (HomeGuard),
\* t3 takes C (CwdGuard); no one takes E then C. LocksBad.tla flips t1 to
\* <<"E", "C">> as a negative control that MUST deadlock.
Need(t) == IF t = "t1" THEN <<"C", "E">>
           ELSE IF t = "t2" THEN <<"E">>
           ELSE <<"C">>

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
