#![forbid(unsafe_code)]
//! Embedded JavaScript workflow pass, `onEnd`, wait codec, and spec inspection driver.

/// Host-callable entrypoints used by both `BoaHost` and external `JsWorkerHost`.
pub(crate) const PRELUDE_DRIVER_JS: &str = r#"
(function(global) {
  if (global.__legionDriverInstalled) return;
  global.__legionDriverInstalled = true;

  const { driveIterator, validateSchema, stepAsyncOp, replayWaitOp, toMillis } = global.__legionInternal;
  const BUILTINS = global.__legionSchemas;
  Object.assign(global.Legion, {
    Issue: BUILTINS.Issue,
    IssueDetail: BUILTINS.IssueDetail,
    PullRequest: BUILTINS.PullRequest,
    PullRequestStanding: BUILTINS.PullRequestStanding,
    Tick: BUILTINS.Tick,
    WebhookRequest: BUILTINS.WebhookRequest,
    every(duration) {
      return { kind: "every", id: "every_" + toMillis(duration), everyMs: toMillis(duration) };
    },
    daily(time, opts) {
      return {
        kind: "daily",
        id: "daily_" + String(time),
        timeOfDay: String(time),
        weekdays: opts && Array.isArray(opts.weekdays) ? opts.weekdays : null
      };
    },
    webhook(opts) {
      const o = opts || {};
      const keyFn = typeof o.key === "function" ? o.key : null;
      const fnIdx = keyFn ? (global.__legionWebhookFns.push(keyFn) - 1) : null;
      return {
        kind: "webhook",
        id: o.id ? String(o.id) : "webhook",
        name: o.name ? String(o.name) : "webhook",
        secret: o.secret ? String(o.secret) : undefined,
        schema: o.schema && o.schema.__schema ? o.schema.__schema : null,
        hasKeyFn: fnIdx !== null,
        keyFnIndex: fnIdx
      };
    },
    issues: {
      opened(opts) {
        const o = opts || {};
        return { kind: "issues_opened", id: o.id ? String(o.id) : "issues_opened", project: String(o.project || ""), labels: Array.isArray(o.labels) ? o.labels : [], query: o.query || null };
      },
      list(filter) { return stepAsyncOp("issues.list", filter || {}); },
      read(issue) { return stepAsyncOp("issues.read", { issue }); },
      comment(issue, body) { return stepAsyncOp("issues.comment", { issue, body: String(body) }); },
      setState(issue, state) { return stepAsyncOp("issues.setState", { issue, state: String(state) }); },
      create(input) { return stepAsyncOp("issues.create", input || {}); }
    },
    pullRequests: {
      opened(opts) {
        const o = opts || {};
        return { kind: "pull_requests_opened", id: o.id ? String(o.id) : "pull_requests_opened", project: String(o.project || ""), query: o.query || null };
      },
      list(filter) { return stepAsyncOp("pullRequests.list", filter || {}); },
      read(pr) { return stepAsyncOp("pullRequests.read", { pr }); },
      comment(pr, body) { return stepAsyncOp("pullRequests.comment", { pr, body: String(body) }); },
      act(pr, action, message) { return stepAsyncOp("pullRequests.act", { pr, action: String(action), message: message || null }); },
      wait(name, pr, opts) {
        const o = opts || {};
        const timeoutMs = o.timeout !== undefined ? toMillis(o.timeout) : undefined;
        return replayWaitOp("prWait", name, { pr, until: o.until || o, timeoutMs });
      }
    }
  });

  function isTargetStepDone(v) {
    return v !== null && typeof v === "object" && v.__legionTargetStepDone === true;
  }

  function runBodyValue(ret, state) {
    if (ret && typeof ret.next === "function") return driveIterator(ret, state);
    if (ret && ret.__legionEffect === true) return ret._exec(state);
    return { ok: true, value: ret };
  }

  function runDriverInternal(fn, args, state) {
    try {
      const ret = fn.apply(undefined, args);
      const res = runBodyValue(ret, state);
      if (res && res.__legionControl) return { status: res.__legionControl, reason: res.reason };
      if (res && res.suspend) return { status: "suspend", ops: res.ops };
      if (res && res.ok) return { status: "completed", value: res.value };
      return { status: "fault", error: String(res && res.error ? res.error : "Workflow failed") };
    } catch (err) {
      if (isTargetStepDone(err)) {
        return { status: "targetStepDone", ok: err.ok, value: err.value, error: err.error };
      }
      if (err && err.__legionControl) return { status: err.__legionControl, reason: err.reason };
      return { status: "fault", error: err instanceof Error ? err.message : String(err) };
    }
  }

  global.__legionDrivePass = function(wfKey, input, journal, targetStep, stepOps, runId) {
    const wf = global.__legionWorkflows[wfKey];
    if (!wf) return { status: "fault", error: "Workflow definition not found: " + wfKey };
    const state = {
      journal: journal || {},
      targetStep: targetStep || null,
      stepOps: stepOps || {},
      inStep: null,
      stepSeq: 0,
      titleSeq: 0
    };
    global.__legionPassState = state;
    const out = runDriverInternal(wf._def.run, [input, { id: runId || "" }], state);
    if (out && out.status === "completed" && wf._def.returns) {
      try {
        out.value = validateSchema(wf._def.returns, out.value, "returns");
      } catch (e) {
        return { status: "fault", error: e instanceof Error ? e.message : String(e) };
      }
    }
    return out;
  };

  global.__legionDriveOnEnd = function(wfKey, input, endInfo, stepOps) {
    const wf = global.__legionWorkflows[wfKey];
    if (!wf || !wf._def.onEnd) return { status: "completed", value: null };
    const state = {
      journal: {},
      targetStep: null,
      stepOps: stepOps || {},
      inStep: "__end__",
      stepSeq: 0,
      titleSeq: 0
    };
    global.__legionPassState = state;
    return runDriverInternal(wf._def.onEnd, [input, endInfo], state);
  };

  global.__legionCallWebhookKey = function(idx, req) {
    const fn = (global.__legionWebhookFns || [])[idx];
    if (typeof fn !== "function") return null;
    const out = fn(req);
    return out === undefined || out === null ? null : String(out);
  };

  global.__legionCheckWaitCodec = function(idx, val) {
    const fn = (global.__legionWaitCodecs || [])[idx];
    if (typeof fn !== "function") return { ok: true, value: val };
    try {
      return { ok: true, value: fn(val) };
    } catch (e) {
      return { ok: false, error: e instanceof Error ? e.message : String(e) };
    }
  };

  global.__legionListWorkflowSpecs = function() {
    return Object.keys(global.__legionWorkflows).map((k) => {
      const w = global.__legionWorkflows[k];
      return {
        name: w.name,
        version: w.version,
        payloadSchema: w.payloadSchema,
        returnsSchema: w.returnsSchema,
        resumeAfterMs: w.resumeAfterMs,
        triggers: w.triggers,
        hasOnEnd: w.hasOnEnd
      };
    });
  };
})(globalThis);
"#;
