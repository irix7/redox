/* The port has no unwinder: Rust std's backtrace printer links these
 * symbols from libgcc_s. They report an empty backtrace. */
typedef int _Unwind_Reason_Code;
typedef int (*_Unwind_Trace_Fn)(void *, void *);

_Unwind_Reason_Code _Unwind_Backtrace(_Unwind_Trace_Fn trace, void *arg) {
    (void)trace;
    (void)arg;
    return 5; /* _URC_END_OF_STACK */
}

unsigned long _Unwind_GetIP(void *context) {
    (void)context;
    return 0;
}

void *_Unwind_FindEnclosingFunction(void *pc) {
    (void)pc;
    return 0;
}

unsigned long _Unwind_GetCFA(void *context) {
    (void)context;
    return 0;
}

unsigned long _Unwind_GetIPInfo(void *context, int *ip_before_insn) {
    (void)context;
    if (ip_before_insn) *ip_before_insn = 0;
    return 0;
}
