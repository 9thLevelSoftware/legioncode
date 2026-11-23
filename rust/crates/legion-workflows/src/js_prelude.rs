#![forbid(unsafe_code)]
//! Embedded JavaScript runtime prelude for `Effect`, `Schema`, `Option`, `Duration`, `Schedule`.

/// Core `Effect`, `Schema`, `Option`, `Duration`, `Schedule`, and generator runner prelude.
pub(crate) const PRELUDE_CORE_JS: &str = r#"
(function(global) {
  if (global.__legionCoreInstalled) return;
  global.__legionCoreInstalled = true;

  function isPlainObject(v) {
    return v !== null && typeof v === "object" && !Array.isArray(v);
  }

  function makeSuspend(ops) {
    return { __legionSuspend: true, ops: ops };
  }

  function isSuspend(v) {
    return v !== null && typeof v === "object" && v.__legionSuspend === true;
  }

  function isTargetStepDone(v) {
    return v !== null && typeof v === "object" && v.__legionTargetStepDone === true;
  }

  function effectProto(exec) {
    return {
      __legionEffect: true,
      _exec: exec,
      pipe() {
        let acc = this;
        for (let i = 0; i < arguments.length; i++) {
          acc = arguments[i](acc);
        }
        return acc;
      },
      [Symbol.iterator]() {
        let yielded = false;
        const self = this;
        return {
          next(val) {
            if (!yielded) {
              yielded = true;
              return { value: self, done: false };
            }
            return { value: val, done: true };
          },
          throw(err) {
            throw err;
          },
          return(val) {
            return { value: val, done: true };
          }
        };
      }
    };
  }

  function runEffectSyncOrSuspend(eff, state) {
    if (!eff || eff.__legionEffect !== true) {
      if (eff && typeof eff.then === "function") {
        throw new Error("Unexpected async Promise returned inside workflow; use Effect primitives or step/agent/wait.");
      }
      if (eff && typeof eff.next === "function") {
        return driveIterator(eff, state);
      }
      return { ok: true, value: eff };
    }
    return eff._exec(state);
  }

  function driveIterator(iter, state) {
    let stepRes;
    let nextInput = undefined;
    let throwing = false;
    while (true) {
      try {
        stepRes = throwing ? iter.throw(nextInput) : iter.next(nextInput);
      } catch (err) {
        if (isSuspend(err)) return { suspend: true, ops: err.ops };
        if (isTargetStepDone(err)) throw err;
        return { ok: false, error: err instanceof Error ? err.message : String(err), rawError: err };
      }
      throwing = false;
      if (stepRes.done) {
        return { ok: true, value: stepRes.value };
      }
      const yielded = stepRes.value;
      const out = runEffectSyncOrSuspend(yielded, state);
      if (out && out.__legionControl) {
        return out;
      }
      if (out && out.suspend) {
        return out;
      }
      if (!out || out.ok) {
        nextInput = out ? out.value : undefined;
      } else {
        throwing = true;
        nextInput = out.rawError !== undefined ? out.rawError : new Error(String(out.error || "Effect failed"));
      }
    }
  }

  function unwrapRes(res) {
    if (res && res.suspend) throw makeSuspend(res.ops);
    if (!res.ok) throw (res.rawError !== undefined ? res.rawError : new Error(String(res.error)));
    return res.value;
  }

  const Option = {
    some(value) { return { _tag: "Some", value }; },
    none() { return { _tag: "None" }; },
    isSome(o) { return Boolean(o && o._tag === "Some"); },
    isNone(o) { return Boolean(!o || o._tag === "None"); },
    getOrElse(o, fn) { return Option.isSome(o) ? o.value : fn(); },
    getOrUndefined(o) { return Option.isSome(o) ? o.value : undefined; },
    fromNullable(v) { return v === null || v === undefined ? Option.none() : Option.some(v); }
  };

  function toMillis(d) {
    if (typeof d === "number" && Number.isFinite(d)) return Math.max(0, Math.floor(d));
    if (d && typeof d.__millis === "number") return d.__millis;
    if (typeof d === "string") {
      const m = /^(\d+(?:\.\d+)?)\s*(ms|millis|millisecond|milliseconds|s|sec|secs|second|seconds|m|min|mins|minute|minutes|h|hr|hrs|hour|hours|d|day|days)?$/i.exec(d.trim());
      if (m) {
        const n = Number(m[1]);
        const u = (m[2] || "ms").toLowerCase();
        const mul = u.startsWith("d") ? 86400000
          : u.startsWith("h") ? 3600000
          : (u === "m" || u.startsWith("min")) ? 60000
          : u.startsWith("s") ? 1000
          : 1;
        return Math.max(0, Math.floor(n * mul));
      }
    }
    throw new Error("Invalid Duration value: " + String(d));
  }

  const Duration = {
    millis(n) { return { _tag: "Duration", __millis: Math.max(0, Math.floor(n)) }; },
    seconds(n) { return Duration.millis(n * 1000); },
    minutes(n) { return Duration.millis(n * 60_000); },
    hours(n) { return Duration.millis(n * 3_600_000); },
    days(n) { return Duration.millis(n * 86_400_000); },
    toMillis
  };

  const Schedule = {
    spaced(d) { return { _tag: "Schedule", times: 3, delayMs: toMillis(d), factor: 1 }; },
    exponential(d, factor) { return { _tag: "Schedule", times: 3, delayMs: toMillis(d), factor: factor || 2 }; },
    recurs(times) { return { _tag: "Schedule", times: Math.max(0, Math.floor(times)), delayMs: 250, factor: 1 }; },
    upTo(s, maxD) { return Object.assign({}, s, { maxDelayMs: toMillis(maxD) }); },
    intersect(a, b) { return Object.assign({}, a, { times: b.times !== undefined ? b.times : a.times }); }
  };

  function makeSchema(jsonSchema, validateFn, optionalFlag) {
    const s = {
      __isLegionSchema: true,
      __schema: jsonSchema,
      __optional: Boolean(optionalFlag),
      _validate: validateFn,
      pipe() {
        let acc = this;
        for (let i = 0; i < arguments.length; i++) acc = arguments[i](acc);
        return acc;
      },
      annotations(meta) {
        return makeSchema(Object.assign({}, jsonSchema, meta), validateFn, optionalFlag);
      }
    };
    return s;
  }

  function validateSchema(s, v, path) {
    if (!s || !s.__isLegionSchema) return v;
    if (s._validate) return s._validate(v, path || "$");
    return v;
  }

  function checkPrimitive(expectedType, checkFn, extra) {
    return makeSchema(Object.assign({ type: expectedType }, extra || {}), (v, path) => {
      if (!checkFn(v)) throw new Error("Expected " + expectedType + " at " + path);
      return v;
    });
  }

  const Schema = {
    String: checkPrimitive("string", (v) => typeof v === "string"),
    NonEmptyString: makeSchema({ type: "string", minLength: 1 }, (v, p) => {
      if (typeof v !== "string" || v.length === 0) throw new Error("Expected non-empty string at " + p);
      return v;
    }),
    Number: checkPrimitive("number", (v) => typeof v === "number" && Number.isFinite(v)),
    Finite: makeSchema({ type: "number" }, (v, p) => {
      if (typeof v !== "number" || !Number.isFinite(v)) throw new Error("Expected finite number at " + p);
      return v;
    }),
    Json: makeSchema({}, (v) => v),
    FiniteFromString: makeSchema({ type: "string" }, (v, p) => {
      if (typeof v !== "string" || v.trim() === "") throw new Error("Expected numeric string at " + p);
      const n = Number(v);
      if (!Number.isFinite(n)) throw new Error("Expected finite number string at " + p);
      return n;
    }),
    NumberFromString: makeSchema({ type: "string" }, (v, p) => {
      if (typeof v !== "string" || v.trim() === "") throw new Error("Expected numeric string at " + p);
      const n = Number(v);
      if (!Number.isFinite(n)) throw new Error("Expected finite number string at " + p);
      return n;
    }),
    Int: makeSchema({ type: "integer" }, (v, p) => {
      if (typeof v !== "number" || !Number.isInteger(v)) throw new Error("Expected integer at " + p);
      return v;
    }),
    Boolean: checkPrimitive("boolean", (v) => typeof v === "boolean"),
    Null: checkPrimitive("null", (v) => v === null),
    Undefined: makeSchema({}, (v, p) => {
      if (v !== undefined) throw new Error("Expected undefined at " + p);
      return v;
    }),
    Void: makeSchema({}, () => undefined),
    Unknown: makeSchema({}, (v) => v),
    Any: makeSchema({}, (v) => v),
    Literals(vals) {
      return Schema.Literal.apply(null, Array.isArray(vals) ? vals : Array.prototype.slice.call(arguments));
    },
    Literal() {
      const vals = Array.prototype.slice.call(arguments);
      const js = vals.length === 1 ? { const: vals[0] } : { enum: vals };
      return makeSchema(js, (v, p) => {
        if (!vals.includes(v)) throw new Error("Expected literal " + JSON.stringify(vals) + " at " + p);
        return v;
      });
    },
    Struct(fields) {
      const props = {};
      const required = [];
      for (const k of Object.keys(fields)) {
        const fs = fields[k];
        props[k] = fs && fs.__schema ? fs.__schema : {};
        if (!fs || !fs.__optional) required.push(k);
      }
      const js = { type: "object", properties: props, required, additionalProperties: false };
      return makeSchema(js, (v, p) => {
        if (!isPlainObject(v)) throw new Error("Expected object at " + p);
        const out = {};
        for (const k of Object.keys(fields)) {
          const fs = fields[k];
          if (v[k] === undefined) {
            if (!fs.__optional) throw new Error("Missing field " + p + "." + k);
          } else {
            out[k] = validateSchema(fs, v[k], p + "." + k);
          }
        }
        return out;
      });
    },
    Array(elem) {
      return makeSchema({ type: "array", items: elem.__schema }, (v, p) => {
        if (!Array.isArray(v)) throw new Error("Expected array at " + p);
        return v.map((item, idx) => validateSchema(elem, item, p + "[" + idx + "]"));
      });
    },
    Tuple() {
      const elems = Array.prototype.slice.call(arguments);
      const js = { type: "array", prefixItems: elems.map((e) => e.__schema), minItems: elems.length, maxItems: elems.length };
      return makeSchema(js, (v, p) => {
        if (!Array.isArray(v) || v.length !== elems.length) throw new Error("Expected tuple of length " + elems.length + " at " + p);
        return elems.map((e, idx) => validateSchema(e, v[idx], p + "[" + idx + "]"));
      });
    },
    Record(opts) {
      const vs = opts.value || opts;
      return makeSchema({ type: "object", additionalProperties: vs.__schema }, (v, p) => {
        if (!isPlainObject(v)) throw new Error("Expected record at " + p);
        const out = {};
        for (const k of Object.keys(v)) out[k] = validateSchema(vs, v[k], p + "." + k);
        return out;
      });
    },
    Union() {
      const members = Array.prototype.slice.call(arguments);
      return makeSchema({ anyOf: members.map((m) => m.__schema) }, (v, p) => {
        for (const m of members) {
          try { return validateSchema(m, v, p); } catch (_) {}
        }
        throw new Error("Value did not match union at " + p);
      });
    },
    optional(inner) {
      return makeSchema(inner.__schema, (v, p) => v === undefined ? undefined : validateSchema(inner, v, p), true);
    },
    NullOr(inner) {
      return Schema.Union(inner, Schema.Null);
    },
    UndefinedOr(inner) {
      return makeSchema(inner.__schema, (v, p) => v === undefined ? undefined : validateSchema(inner, v, p));
    },
    isFinite() {
      return (s) => makeSchema(s.__schema, (v, p) => {
        const out = validateSchema(s, v, p);
        if (typeof out !== "number" || !Number.isFinite(out)) throw new Error("Expected finite number at " + p);
        return out;
      });
    },
    annotations(meta) {
      return (s) => s.annotations(meta);
    },
    decodeUnknownSync(s) {
      return (v) => validateSchema(s, v, "$");
    },
    decodeUnknown(s) {
      return (v) => Effect.try(() => validateSchema(s, v, "$"));
    },
    encodeSync(s) {
      return (v) => validateSchema(s, v, "$");
    },
    is(s) {
      return (v) => { try { validateSchema(s, v, "$"); return true; } catch (_) { return false; } };
    }
  };

  global.Schema = Schema;
  global.Option = Option;
  global.Duration = Duration;
  global.Schedule = Schedule;
  global.__legionInternal = {
    effectProto,
    runEffectSyncOrSuspend,
    driveIterator,
    makeSuspend,
    isSuspend,
    unwrapRes,
    validateSchema,
    toMillis
  };
})(globalThis);
"#;
