#![forbid(unsafe_code)]
//! Built-in JS `Schema` definitions attached to `Legion` and `T3`.

/// Built-in schema objects exposed on `Legion`/`T3` (`Issue`, `PullRequest`, `Tick`, etc.).
pub(crate) const PRELUDE_SCHEMAS_JS: &str = r#"
(function(global) {
  if (global.__legionSchemasInstalled) return;
  global.__legionSchemasInstalled = true;
  const {
    effectProto,
    runEffectSyncOrSuspend,
    driveIterator,
    isSuspend,
    toMillis
  } = global.__legionInternal;
  const Option = global.Option;
  const Effect = {
    succeed(value) { return effectProto(() => ({ ok: true, value })); },
    fail(err) {
      return effectProto(() => ({
        ok: false,
        error: err instanceof Error ? err.message : String(err),
        rawError: err
      }));
    },
    sync(fn) { return effectProto(() => ({ ok: true, value: fn() })); },
    try(arg) {
      const fn = typeof arg === "function" ? arg : arg.try;
      const mapErr = typeof arg === "function" ? null : arg.catch;
      return effectProto(() => {
        try {
          return { ok: true, value: fn() };
        } catch (err) {
          if (isSuspend(err)) throw err;
          const mapped = mapErr ? mapErr(err) : err;
          return { ok: false, error: mapped instanceof Error ? mapped.message : String(mapped), rawError: mapped };
        }
      });
    },
    tryPromise(arg) { return Effect.try(arg); },
    promise(fn) { return Effect.try(fn); },
    gen(fn) {
      return effectProto((state) => {
        const iter = fn(function(eff) { return eff; });
        return driveIterator(iter, state);
      });
    },
    sleep(d) {
      const ms = toMillis(d);
      return global.Legion.sleep("sleep_" + ms, ms);
    },
    all(items, _opts) {
      return effectProto((state) => {
        if (Array.isArray(items)) {
          const out = new Array(items.length);
          const pending = [];
          for (let i = 0; i < items.length; i++) {
            const res = runEffectSyncOrSuspend(items[i], state);
            if (res && res.suspend) {
              for (const op of res.ops) pending.push(op);
            } else if (!res.ok) {
              if (pending.length === 0) return res;
            } else {
              out[i] = res.value;
            }
          }
          if (pending.length > 0) return { suspend: true, ops: pending };
          return { ok: true, value: out };
        }
        const out = {};
        const pending = [];
        for (const k of Object.keys(items || {})) {
          const res = runEffectSyncOrSuspend(items[k], state);
          if (res && res.suspend) {
            for (const op of res.ops) pending.push(op);
          } else if (!res.ok) {
            if (pending.length === 0) return res;
          } else {
            out[k] = res.value;
          }
        }
        if (pending.length > 0) return { suspend: true, ops: pending };
        return { ok: true, value: out };
      });
    },
    forEach(items, fn, opts) {
      return effectProto((state) => Effect.all(Array.from(items).map((x, i) => fn(x, i)), opts)._exec(state));
    },
    map(eff, fn) {
      if (arguments.length === 1) return (target) => Effect.map(target, eff);
      return effectProto((state) => {
        const res = runEffectSyncOrSuspend(eff, state);
        if (res && res.suspend) return res;
        if (!res.ok) return res;
        return { ok: true, value: fn(res.value) };
      });
    },
    flatMap(eff, fn) {
      if (arguments.length === 1) return (target) => Effect.flatMap(target, eff);
      return effectProto((state) => {
        const res = runEffectSyncOrSuspend(eff, state);
        if (res && res.suspend) return res;
        if (!res.ok) return res;
        return runEffectSyncOrSuspend(fn(res.value), state);
      });
    },
    tap(eff, fn) {
      if (arguments.length === 1) return (target) => Effect.tap(target, eff);
      return Effect.flatMap(eff, (v) => Effect.map(fn(v), () => v));
    },
    catchAll(eff, fn) {
      if (arguments.length === 1) return (target) => Effect.catchAll(target, eff);
      return effectProto((state) => {
        const res = runEffectSyncOrSuspend(eff, state);
        if (res && res.suspend) return res;
        if (res.ok) return res;
        return runEffectSyncOrSuspend(fn(res.rawError !== undefined ? res.rawError : res.error), state);
      });
    },
    either(eff) {
      return effectProto((state) => {
        const res = runEffectSyncOrSuspend(eff, state);
        if (res && res.suspend) return res;
        return res.ok
          ? { ok: true, value: { _tag: "Right", right: res.value } }
          : { ok: true, value: { _tag: "Left", left: res.rawError !== undefined ? res.rawError : res.error } };
      });
    },
    option(eff) {
      return effectProto((state) => {
        const res = runEffectSyncOrSuspend(eff, state);
        if (res && res.suspend) return res;
        return res.ok ? { ok: true, value: Option.some(res.value) } : { ok: true, value: Option.none() };
      });
    },
    ensuring(eff, finalizer) {
      if (arguments.length === 1) return (target) => Effect.ensuring(target, eff);
      return effectProto((state) => {
        const res = runEffectSyncOrSuspend(eff, state);
        if (res && res.suspend) return res;
        const finRes = runEffectSyncOrSuspend(finalizer, state);
        if (finRes && finRes.suspend) return finRes;
        return res;
      });
    },
    runSyncExit(eff) {
      return runEffectSyncOrSuspend(eff, global.__legionPassState || {});
    }
  };
  global.Effect = Effect;
  const Schema = global.Schema;

  global.__legionSchemas = {
    Issue: Schema.Struct({
      projectId: Schema.String,
      tracker: Schema.String,
      host: Schema.optional(Schema.String),
      scope: Schema.String,
      key: Schema.String,
      number: Schema.Number,
      title: Schema.String,
      state: Schema.Literal("open", "closed"),
      url: Schema.String,
      labels: Schema.Array(Schema.String),
      createdAt: Schema.String,
      updatedAt: Schema.String
    }),
    IssueDetail: Schema.Struct({
      projectId: Schema.String,
      tracker: Schema.String,
      scope: Schema.String,
      key: Schema.String,
      number: Schema.Number,
      title: Schema.String,
      body: Schema.String,
      state: Schema.Literal("open", "closed"),
      url: Schema.String,
      labels: Schema.Array(Schema.String)
    }),
    PullRequest: Schema.Struct({
      projectId: Schema.String,
      tracker: Schema.String,
      scope: Schema.String,
      number: Schema.Number,
      title: Schema.String,
      state: Schema.Literal("open", "closed", "merged"),
      draft: Schema.Boolean,
      merged: Schema.Boolean,
      url: Schema.String,
      headBranch: Schema.String,
      baseBranch: Schema.String
    }),
    PullRequestStanding: Schema.Struct({
      state: Schema.String,
      checks: Schema.String,
      mergeability: Schema.String,
      url: Schema.String
    }),
    Tick: Schema.Struct({
      triggerId: Schema.String,
      firedAt: Schema.String,
      scheduledFor: Schema.String
    }),
    WebhookRequest: Schema.Struct({
      hookId: Schema.String,
      method: Schema.String,
      headers: Schema.Record({ key: Schema.String, value: Schema.String }),
      query: Schema.Record({ key: Schema.String, value: Schema.String }),
      body: Schema.Unknown,
      rawBody: Schema.String
    })
  };
})(globalThis);
"#;
