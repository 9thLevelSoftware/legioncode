#![forbid(unsafe_code)]
//! Embedded JavaScript `Legion` / `T3` workflow namespace and step/replay primitives.

/// `Legion` and `T3` namespace definitions exposed in workflow runtimes.
pub(crate) const PRELUDE_API_JS: &str = r#"
(function(global) {
  if (global.__legionApiInstalled) return;
  global.__legionApiInstalled = true;

  const {
    effectProto,
    runEffectSyncOrSuspend,
    validateSchema,
    toMillis
  } = global.__legionInternal;
  const Schema = global.Schema;
  const Effect = global.Effect;
  const BUILTINS = global.__legionSchemas;

  global.__legionWorkflows = global.__legionWorkflows || Object.create(null);
  global.__legionWebhookFns = global.__legionWebhookFns || [];

  function makeStepTargetDone(ok, value, error) {
    return { __legionTargetStepDone: true, ok: Boolean(ok), value, error };
  }

  function normalizeRetry(retry) {
    if (!retry) return { times: 0, delayMs: 250 };
    if (typeof retry === "number") return { times: Math.max(0, Math.floor(retry)), delayMs: 250 };
    if (retry._tag === "Schedule") {
      return { times: retry.times || 0, delayMs: retry.delayMs || 250 };
    }
    const sched = retry.schedule;
    return {
      times: typeof retry.times === "number" ? Math.max(0, Math.floor(retry.times)) : 0,
      delayMs: sched && typeof sched.delayMs === "number" ? sched.delayMs
        : retry.delay !== undefined ? toMillis(retry.delay)
        : (typeof retry.delayMs === "number" ? retry.delayMs : 250)
    };
  }

  function parseSignalAddress(target, maybeChannel) {
    if (typeof target === "string" && typeof maybeChannel === "string") {
      return global.__legionWorkflows[target]
        ? { workflow: target, channel: maybeChannel }
        : { runId: target, wait: maybeChannel };
    }
    if (target && target.__isLegionWorkflow && typeof maybeChannel === "string") {
      return { workflow: target.name, channel: maybeChannel };
    }
    if (target && typeof target.runId === "string" && typeof target.wait === "string") {
      return { runId: target.runId, wait: target.wait };
    }
    if (target && typeof target.workflow === "string" && typeof target.channel === "string") {
      return { workflow: target.workflow, channel: target.channel };
    }
    throw new Error("Invalid signal address target: " + String(target));
  }

  function parseStepArgs(name, optsOrCode, maybeCode) {
    if (maybeCode === undefined) {
      return { name, opts: {}, code: optsOrCode };
    }
    return { name, opts: optsOrCode || {}, code: maybeCode };
  }

  function makeStepEffect(name, optsOrCode, maybeCode, captureError) {
    const parsed = parseStepArgs(name, optsOrCode, maybeCode);
    const retry = normalizeRetry(parsed.opts.retry);
    const schemaObj = parsed.opts.returns || parsed.opts.schema || null;
    const schemaJson = schemaObj && schemaObj.__schema ? schemaObj.__schema : null;
    return effectProto((state) => {
      if (state.inStep) {
        throw new Error("Nested step() calls are not allowed inside step '" + state.inStep + "'.");
      }
      const entry = state.journal ? state.journal["step/" + parsed.name] : undefined;
      if (entry && entry.status === "completed") {
        return { ok: true, value: entry.value };
      }
      if (state.targetStep === parsed.name) {
        state.inStep = parsed.name;
        state.stepSeq = 0;
        let out;
        try {
          const stepCtx = buildStepApi(parsed.name);
          const raw = typeof parsed.code === "function" ? parsed.code(stepCtx) : parsed.code;
          out = runEffectSyncOrSuspend(raw, state);
        } catch (err) {
          out = { ok: false, error: err instanceof Error ? err.message : String(err), rawError: err };
        } finally {
          state.inStep = null;
        }
        if (out && out.suspend) return out;
        if (out && out.ok && schemaObj) {
          try {
            out.value = validateSchema(schemaObj, out.value, "step:" + parsed.name);
          } catch (err) {
            out = { ok: false, error: err instanceof Error ? err.message : String(err) };
          }
        }
        throw makeStepTargetDone(out && out.ok, out ? out.value : undefined, out ? out.error : "step failed");
      }
      if (state.targetStep !== null && state.targetStep !== undefined) {
        return { suspend: true, ops: [] };
      }
      return {
        suspend: true,
        ops: [{ kind: "step", name: parsed.name, retry, schema: schemaJson, captureError }]
      };
    });
  }

  function stepAsyncOp(opName, payload) {
    return effectProto((state) => {
      const stepName = state.inStep || "__top";
      const seq = state.stepSeq || 0;
      state.stepSeq = seq + 1;
      const cacheKey = stepName + ":" + seq;
      if (state.stepOps && Object.prototype.hasOwnProperty.call(state.stepOps, cacheKey)) {
        const cached = state.stepOps[cacheKey];
        if (!cached.ok) {
          return { ok: false, error: cached.error || opName + " failed" };
        }
        return { ok: true, value: cached.value };
      }
      return {
        suspend: true,
        ops: [{ kind: "stepAsync", step: stepName, seq, cacheKey, op: opName, payload: payload || {} }]
      };
    });
  }

  function replayJournalOp(journalKey, kind, name, payload, mapFn) {
    return effectProto((state) => {
      if (state.targetStep !== null && state.targetStep !== undefined) {
        return { suspend: true, ops: [] };
      }
      const entry = state.journal ? state.journal[journalKey] : undefined;
      if (entry && entry.status === "completed") {
        try {
          return { ok: true, value: mapFn ? mapFn(entry.value) : entry.value };
        } catch (err) {
          return { ok: false, error: err instanceof Error ? err.message : String(err) };
        }
      }
      return {
        suspend: true,
        ops: [Object.assign({ kind, name }, payload || {})]
      };
    });
  }

  function replayWaitOp(kind, name, payload, validatorFn) {
    return replayJournalOp("wait/" + name, kind, name, payload, (rec) => {
      if (!rec || rec.resolved !== true) return global.Option.none();
      const val = validatorFn ? validatorFn(rec.value) : rec.value;
      return global.Option.some(val);
    });
  }

  function replayAgentOp(name, opts, tryMode) {
    return replayJournalOp("agent/" + name, "agent", name, { opts: normalizeAgentOpts(opts), tryMode }, (attempt) => {
      if (tryMode) return attempt;
      if (!attempt || attempt.outcome !== "done") {
        throw new Error((attempt && attempt.reason) || "agent failed");
      }
      return { value: attempt.value, thread: attempt.thread };
    });
  }

  function normalizeAgentOpts(opts) {
    const out = Object.assign({}, opts || {});
    const schema = out.returns || out.schema;
    if (schema && schema.__schema) {
      out.outputSchema = schema.__schema;
      delete out.schema;
      delete out.returns;
    }
    if (out.quiet !== undefined) out.quietMs = toMillis(out.quiet);
    if (out.timeout !== undefined) out.timeoutMs = toMillis(out.timeout);
    return out;
  }

  function normalizeWaitOpts(opts) {
    const o = opts || {};
    const schemaObj = o.returns || o.schema || Schema.Unknown;
    const timeoutMs = o.timeout !== undefined ? toMillis(o.timeout) : undefined;
    const codecIdx = global.__legionWaitCodecs
      ? global.__legionWaitCodecs.push((v) => validateSchema(schemaObj, v, "wait")) - 1
      : null;
    return {
      schemaObj,
      payload: {
        channel: o.channel || null,
        schema: schemaObj.__schema || {},
        timeoutMs,
        codecIdx
      }
    };
  }

  function buildStepApi(stepName) {
    return {
      name: stepName,
      store: api.store,
      signal: api.signal,
      signalAll: api.signalAll,
      start: api.start,
      cancel: api.cancel,
      runs: api.runs,
      issues: api.issues,
      pullRequests: api.pullRequests,
      noteReference: api.noteReference
    };
  }

  const DEFAULT_RESUME_MS = [300_000, 1_200_000, 4_800_000];
  global.__legionWaitCodecs = global.__legionWaitCodecs || [];

  const api = {
    workflow(nameOrObj, maybeOpts, maybeBody) {
      const def = typeof nameOrObj === "string"
        ? Object.assign({}, maybeOpts || {}, { name: nameOrObj, run: maybeBody })
        : nameOrObj;
      if (!def || typeof def !== "object") throw new Error("workflow() requires a definition");
      if (typeof def.name !== "string" || !def.name) throw new Error("workflow name must be a non-empty string");
      if (typeof def.version !== "number" || !Number.isInteger(def.version) || def.version < 1) {
        throw new Error("workflow '" + def.name + "' version must be a positive integer");
      }
      if (typeof def.run !== "function") throw new Error("workflow '" + def.name + "' must define a run function");
      const wfKey = def.name + "@" + def.version;
      if (global.__legionWorkflows[wfKey]) {
        throw new Error("Duplicate workflow definition '" + wfKey + "'");
      }
      const resumeAfterMs = def.resumeAfter === undefined
        ? DEFAULT_RESUME_MS.slice()
        : (Array.isArray(def.resumeAfter) ? def.resumeAfter.map(toMillis) : []);
      const rec = {
        __isLegionWorkflow: true,
        name: def.name,
        version: def.version,
        payloadSchema: def.payload && def.payload.__schema ? def.payload.__schema : {},
        payloadValidator: def.payload || Schema.Unknown,
        returnsSchema: def.returns && def.returns.__schema ? def.returns.__schema : null,
        resumeAfterMs,
        triggers: Array.isArray(def.triggers) ? def.triggers : [],
        hasOnEnd: typeof def.onEnd === "function",
        _def: def
      };
      global.__legionWorkflows[wfKey] = rec;
      return rec;
    },

    step: Object.assign(
      function(name, optsOrCode, maybeCode) { return makeStepEffect(name, optsOrCode, maybeCode, false); },
      { try(name, optsOrCode, maybeCode) { return makeStepEffect(name, optsOrCode, maybeCode, true); } }
    ),

    agent: Object.assign(
      function(name, opts) { return replayAgentOp(name, opts, false); },
      { try(name, opts) { return replayAgentOp(name, opts, true); } }
    ),

    wait(name, opts) {
      const w = normalizeWaitOpts(opts);
      return replayWaitOp("wait", name, w.payload, (v) => validateSchema(w.schemaObj, v, "wait:" + name));
    },

    ask(name, opts) {
      const o = opts || {};
      const timeoutMs = o.timeout !== undefined ? toMillis(o.timeout) : undefined;
      return replayWaitOp("ask", name, {
        title: o.title || name,
        choices: Array.isArray(o.choices) ? o.choices : null,
        data: o.data,
        timeoutMs
      });
    },

    sleep(name, duration) {
      return replayJournalOp("sleep/" + name, "sleep", name, { ms: toMillis(duration) }, () => undefined);
    },

    title(text) {
      return effectProto((state) => {
        if (state.targetStep !== null && state.targetStep !== undefined) return { suspend: true, ops: [] };
        const seq = state.titleSeq || 0;
        state.titleSeq = seq + 1;
        const jkey = "title/title/" + seq;
        if (state.journal && state.journal[jkey] && state.journal[jkey].status === "completed") {
          return { ok: true, value: undefined };
        }
        return { suspend: true, ops: [{ kind: "title", value: String(text) }] };
      });
    },

    stop(reason) { return effectProto(() => ({ __legionControl: "stop", reason: String(reason || "") })); },
    skip(reason) { return effectProto(() => ({ __legionControl: "skip", reason: String(reason || "") })); },

    invoke(name, target, payload, opts) {
      const wfName = typeof target === "string" ? target : target.name;
      const version = typeof target === "object" && typeof target.version === "number" ? target.version : null;
      return replayJournalOp("wait/" + name, "invoke", name, {
        workflow: wfName,
        version,
        payload: payload === undefined ? null : payload,
        key: opts && opts.key ? String(opts.key) : undefined
      }, (rec) => {
        if (!rec || rec.resolved !== true) throw new Error("Child workflow ended without outcome");
        return rec.value;
      });
    },

    runs: Object.assign(
      function(filter) { return stepAsyncOp("runs", filter || {}); },
      {
        at(runId, wait) { return { runId: String(runId), wait: String(wait) }; },
        channel(wf, channel) { return parseSignalAddress(wf, channel); }
      }
    ),

    signal(target, value, opts) {
      const o = opts || {};
      return stepAsyncOp("signal", {
        address: parseSignalAddress(target),
        value: value === undefined ? null : value,
        signalId: o.id,
        ttlMs: o.ttl !== undefined ? toMillis(o.ttl) : undefined
      });
    },

    signalAll(target, value) {
      const addr = typeof target === "string"
        ? { channel: target, workflow: null }
        : { channel: String(target.channel), workflow: target.workflow || null };
      return stepAsyncOp("signalAll", {
        address: addr,
        value: value === undefined ? null : value
      });
    },

    start(target, payload, opts) {
      const wfName = typeof target === "string" ? target : target.name;
      return stepAsyncOp("start", {
        workflow: wfName,
        payload: payload === undefined ? null : payload,
        key: opts && opts.key ? String(opts.key) : undefined,
        version: opts && typeof opts.version === "number" ? opts.version : null
      });
    },

    cancel(runId, reason) {
      return stepAsyncOp("cancel", { runId: String(runId), reason: reason || null });
    },

    noteReference(ref) {
      return stepAsyncOp("noteReference", { reference: ref });
    },

    store: {
      get(key, schema) {
        return Effect.map(stepAsyncOp("store.get", { key: String(key) }), (val) => {
          if (val === undefined || val === null) return undefined;
          return schema && schema.__isLegionSchema ? validateSchema(schema, val, "store:" + key) : val;
        });
      },
      set(key, value) { return stepAsyncOp("store.set", { key: String(key), value: value === undefined ? null : value }); },
      delete(key) { return stepAsyncOp("store.delete", { key: String(key) }); },
      list(prefix) { return stepAsyncOp("store.list", { prefix: prefix || "" }); }
    }
  };

  global.__legionInternal.stepAsyncOp = stepAsyncOp;
  global.__legionInternal.replayWaitOp = replayWaitOp;
  global.Legion = api;
  global.T3 = api;
  global.__legionBuildStepApi = buildStepApi;
})(globalThis);
"#;
