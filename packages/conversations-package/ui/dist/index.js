//#region \0rolldown/runtime.js
var e = Object.defineProperty, t = (t, n) => {
	let r = {};
	for (var i in t) e(r, i, {
		get: t[i],
		enumerable: !0
	});
	return n || e(r, Symbol.toStringTag, { value: "Module" }), r;
};
Object.freeze([
	"--teamarness-color-background",
	"--teamarness-color-panel",
	"--teamarness-color-text",
	"--teamarness-color-text-muted",
	"--teamarness-color-border",
	"--teamarness-color-accent",
	"--teamarness-color-danger",
	"--teamarness-color-focus",
	"--teamarness-font-body",
	"--teamarness-font-mono",
	"--teamarness-radius-control",
	"--teamarness-radius-panel"
]);
var n = "\n  :host, :host * { box-sizing: border-box; }\n\n  /* Inherit the complete surrounding font instead of selecting var(--teamarness-font-body). */\n  .teamarness-control {\n    border: 1px solid var(--teamarness-color-border);\n    border-radius: var(--teamarness-radius-control);\n    background: var(--teamarness-color-panel);\n    color: var(--teamarness-color-text);\n    font: inherit;\n    padding: 0.55rem 0.8rem;\n  }\n  .teamarness-control--primary {\n    border-color: var(--teamarness-color-accent);\n    background: var(--teamarness-color-accent);\n    color: var(--teamarness-color-background);\n  }\n  .teamarness-control--secondary {\n    border-color: var(--teamarness-color-border);\n    background: var(--teamarness-color-panel);\n    color: var(--teamarness-color-text);\n  }\n  .teamarness-field { display: grid; gap: 0.35rem; color: var(--teamarness-color-text); }\n  .teamarness-input {\n    width: 100%;\n    border: 1px solid var(--teamarness-color-border);\n    border-radius: var(--teamarness-radius-control);\n    background: var(--teamarness-color-background);\n    color: var(--teamarness-color-text);\n    font: inherit;\n    padding: 0.55rem 0.65rem;\n  }\n  .teamarness-help { color: var(--teamarness-color-text-muted); }\n  .teamarness-validation { color: var(--teamarness-color-danger); font-family: var(--teamarness-font-mono); }\n  .teamarness-notice {\n    border: 1px solid var(--teamarness-color-border);\n    border-radius: var(--teamarness-radius-control);\n    background: var(--teamarness-color-panel);\n    color: var(--teamarness-color-text);\n    padding: 0.75rem;\n  }\n  .teamarness-notice--danger { border-color: var(--teamarness-color-danger); color: var(--teamarness-color-danger); }\n  .teamarness-panel {\n    border: 1px solid var(--teamarness-color-border);\n    border-radius: var(--teamarness-radius-panel);\n    background: var(--teamarness-color-panel);\n    color: var(--teamarness-color-text);\n    padding: 1rem;\n  }\n  .teamarness-stack { display: flex; flex-direction: column; gap: 0.75rem; }\n  .teamarness-inline { display: flex; flex-wrap: wrap; align-items: center; gap: 0.75rem; }\n  .teamarness-focus:focus-visible,\n  .teamarness-control:focus-visible,\n  .teamarness-input:focus-visible {\n    outline: 3px solid var(--teamarness-color-focus);\n    outline-offset: 2px;\n  }\n", r = /* @__PURE__ */ new WeakMap(), i = /* @__PURE__ */ new WeakMap(), a = {
	autocapitalize: "off",
	autocorrect: "off",
	spellcheck: "false",
	writingsuggestions: "false"
}, o = "input,textarea,[contenteditable]";
function s(e) {
	if (e.nodeType === 1) {
		let t = e, n = t.tagName;
		if (n === "INPUT" || n === "TEXTAREA") for (let [e, n] of Object.entries(a)) t.setAttribute(e, n);
		for (let e of t.querySelectorAll(o)) for (let [t, n] of Object.entries(a)) e.setAttribute(t, n);
	}
}
function c(e) {
	if (i.has(e)) return () => void 0;
	if (typeof MutationObserver != "function") return s(e), () => void 0;
	let t = new MutationObserver((e) => {
		for (let t of e) for (let e of t.addedNodes) s(e);
	});
	t.observe(e, {
		childList: !0,
		subtree: !0
	}), s(e), i.set(e, { observer: t });
	let n = !1;
	return () => {
		n || (n = !0, t.disconnect(), i.delete(e));
	};
}
function l(e) {
	if (!("adoptedStyleSheets" in e) || e.adoptedStyleSheets === void 0) throw Error("The Package UI kit requires ShadowRoot.adoptedStyleSheets support");
	return e.adoptedStyleSheets;
}
function u() {
	let e = globalThis.CSSStyleSheet;
	if (typeof e != "function") throw Error("The Package UI kit requires constructed CSSStyleSheet support");
	let t;
	try {
		t = new e();
	} catch (e) {
		throw Error("The Package UI kit requires constructed CSSStyleSheet support", { cause: e });
	}
	if (typeof t.replaceSync != "function") throw Error("The Package UI kit requires CSSStyleSheet.replaceSync support");
	return t.replaceSync(n), t;
}
function d(e, t) {
	try {
		e.adoptedStyleSheets = t;
	} catch (e) {
		throw Error("The Package UI kit requires mutable ShadowRoot.adoptedStyleSheets support", { cause: e });
	}
}
function f(e) {
	let t = r.get(e);
	if (t === void 0) {
		let n = u();
		if (d(e, [...l(e), n]), !l(e).includes(n)) throw Error("The Package UI kit requires mutable ShadowRoot.adoptedStyleSheets support");
		t = {
			sheet: n,
			users: 0
		}, r.set(e, t);
	}
	t.users += 1;
	let n = c(e), i = !1;
	return () => {
		if (i) return;
		i = !0;
		let a = r.get(e);
		a === t && (--a.users, a.users === 0 && (n(), d(e, l(e).filter((e) => e !== a.sheet)), r.delete(e)));
	};
}
function p(e) {
	if (typeof e == "object" && e) {
		let t = e;
		if (typeof t.code == "string" && t.code.length > 0) return {
			code: t.code,
			retryable: t.retryable === !0
		};
	}
	return {
		code: "unknown_error",
		retryable: !1
	};
}
function m(e, t, n) {
	if (console.debug("Conversations operation failed", {
		operation: e,
		code: t.code,
		retryable: t.retryable
	}), n !== "current") return "Conversations is catching up. Your message will be sent soon.";
	if (t.retryable) return "Unable to complete the action. Please try again.";
	switch (t.code) {
		case "conversations.channel_hidden": return "This Channel has been permanently hidden.";
		case "conversations.invalid_slug": return "Channel names use lowercase letters, numbers, and hyphens.";
		case "conversations.invalid_markdown": return "Message content is too long or unsupported.";
		case "conversations.confirmation_required": return "Please confirm this action before proceeding.";
		default: return "Something went wrong. Please try again.";
	}
}
var h = class {
	_state;
	generation = 0;
	disposed = !1;
	client;
	onChange;
	draft = "";
	constructor(e, t) {
		this.client = e, this.onChange = t, this._state = {
			channels: [],
			selectedChannelId: null,
			selectedSlug: null,
			selectedCanHide: !1,
			messages: [],
			hasMore: !1,
			nextBefore: null,
			loading: !0,
			loadingOlder: !1,
			availability: "catching_up",
			warning: null,
			warnings: [],
			creating: !1,
			confirmingHide: !1
		};
	}
	get state() {
		return this._state;
	}
	async initialize() {
		if (this.disposed) return;
		try {
			let e = await this.client.request("conversations.snapshot", {});
			this.applySnapshotSync(e);
		} catch {
			this._state = {
				...this._state,
				loading: !1,
				availability: "degraded",
				warning: "Conversations is temporarily unavailable."
			}, this.notify();
			return;
		}
		let e = this._state.selectedChannelId;
		if (e) try {
			let t = await this.client.request("conversations.channel.open", {
				channelId: e,
				limit: 32
			});
			if (this.disposed) return;
			this._state = {
				...this._state,
				messages: t.messages,
				hasMore: t.nextBefore !== void 0,
				nextBefore: t.nextBefore ?? null,
				loading: !1
			};
		} catch {
			if (this.disposed) return;
			this._state = {
				...this._state,
				loading: !1,
				warning: "Conversation history is temporarily unavailable."
			};
		} finally {
			this.disposed || this.notify();
		}
	}
	applySnapshotSync(e) {
		let t = e.channels, n = e.availability ?? "current", r = t.find((e) => e.slug === "general")?.id ?? t[0]?.id ?? null;
		this._state = {
			...this._state,
			channels: t,
			availability: n,
			warnings: e.warnings ?? [],
			selectedChannelId: r,
			loading: r !== null && this._state.messages.length === 0
		};
	}
	dispose() {
		this.disposed = !0, this.generation = 0;
	}
	async selectChannel(e) {
		if (this.disposed || this._state.selectedChannelId === e) return;
		let t = ++this.generation;
		this._state = {
			...this._state,
			selectedChannelId: e,
			messages: [],
			hasMore: !1,
			nextBefore: null,
			loading: !0,
			loadingOlder: !1,
			warning: null,
			confirmingHide: !1
		}, this.notify();
		try {
			let n = await this.client.request("conversations.channel.open", {
				channelId: e,
				limit: 32
			});
			if (this.disposed || this.generation !== t) return;
			this._state = {
				...this._state,
				messages: n.messages,
				hasMore: n.nextBefore !== void 0,
				nextBefore: n.nextBefore ?? null,
				loading: !1
			};
		} catch (e) {
			if (this.disposed || this.generation !== t) return;
			let n = p(e);
			this._state = {
				...this._state,
				loading: !1,
				warning: m("channel.open", n, this._state.availability)
			};
		} finally {
			!this.disposed && this.generation === t && this.notify();
		}
	}
	async loadOlder() {
		if (this.disposed) return;
		let { hasMore: e, nextBefore: t, loadingOlder: n, selectedChannelId: r } = this._state;
		if (e && t && !n && r) {
			this._state = {
				...this._state,
				loadingOlder: !0,
				warning: null
			}, this.notify();
			try {
				let e = await this.client.request("conversations.channel.page", {
					channelId: r,
					before: t,
					limit: 32
				});
				if (this.disposed) return;
				let n = new Set(this._state.messages.map((e) => e.id)), i = e.messages.filter((e) => !n.has(e.id));
				this._state = {
					...this._state,
					messages: [...this._state.messages, ...i],
					hasMore: e.nextBefore !== void 0,
					nextBefore: e.nextBefore ?? null,
					loadingOlder: !1
				};
			} catch {
				if (this.disposed) return;
				this._state = {
					...this._state,
					loadingOlder: !1,
					warning: "Older history could not be loaded."
				};
			} finally {
				this.disposed || this.notify();
			}
		}
	}
	async createChannel(e) {
		if (!this.disposed) {
			this._state = {
				...this._state,
				loading: !0,
				warning: null
			}, this.notify();
			try {
				if (await this.client.request("conversations.channel.create", { slug: e }), this.disposed) return;
				let t = await this.client.request("conversations.snapshot", {});
				if (this.disposed) return;
				this._state = {
					...this._state,
					loading: !1,
					creating: !1
				}, this.applySnapshot(t);
			} catch {
				if (this.disposed) return;
				this._state = {
					...this._state,
					loading: !1,
					warning: "Channel names use lowercase letters, numbers, and hyphens."
				}, this.notify();
			}
		}
	}
	async hideChannel() {
		if (this.disposed) return;
		let e = this._state.selectedChannelId;
		if (e) {
			this._state = {
				...this._state,
				loading: !0,
				warning: null
			}, this.notify();
			try {
				if (await this.client.request("conversations.channel.hide", {
					channelId: e,
					confirm: !0
				}), this.disposed) return;
				let t = await this.client.request("conversations.snapshot", {});
				if (this.disposed) return;
				this._state = {
					...this._state,
					loading: !1,
					confirmingHide: !1
				}, this.applySnapshot(t);
			} catch {
				if (this.disposed) return;
				this._state = {
					...this._state,
					loading: !1,
					warning: "Only the Channel creator can permanently hide this Channel."
				}, this.notify();
			}
		}
	}
	async postMessage(e) {
		if (this.disposed) return;
		let t = this._state.selectedChannelId;
		if (t && e.trim()) {
			this._state = {
				...this._state,
				loading: !0,
				warning: null
			}, this.notify();
			try {
				if (await this.client.request("conversations.message.post", {
					channelId: t,
					markdown: e
				}), this.disposed) return;
				let n = await this.client.request("conversations.channel.open", {
					channelId: t,
					limit: 32
				});
				if (this.disposed) return;
				this._state = {
					...this._state,
					messages: n.messages,
					hasMore: n.nextBefore !== void 0,
					nextBefore: n.nextBefore ?? null,
					loading: !1
				};
			} catch (e) {
				if (this.disposed) return;
				let t = p(e);
				this._state = {
					...this._state,
					loading: !1,
					warning: m("message.post", t, this._state.availability)
				};
			} finally {
				this.disposed || this.notify();
			}
		}
	}
	applySnapshot(e) {
		if (this.disposed) return;
		let t = e.channels, n = e.availability ?? "current", r = t.find((e) => e.slug === "general"), i = this._state.selectedChannelId !== null && t.some((e) => e.id === this._state.selectedChannelId) ? this._state.selectedChannelId : r?.id ?? t[0]?.id ?? null;
		if (i !== this._state.selectedChannelId) {
			this._state = {
				...this._state,
				channels: t,
				availability: n,
				warnings: e.warnings ?? [],
				selectedChannelId: i,
				messages: [],
				hasMore: !1,
				nextBefore: null,
				loading: !0,
				warning: null
			}, this.notify(), i && this.selectChannel(i);
			return;
		}
		return this._state = {
			...this._state,
			channels: t,
			availability: n,
			warnings: e.warnings ?? []
		}, this.notify(), this.reloadCurrentChannel();
	}
	async reloadCurrentChannel() {
		let e = this._state.selectedChannelId;
		if (e) try {
			let t = await this.client.request("conversations.channel.open", {
				channelId: e,
				limit: 32
			});
			if (this.disposed) return;
			this._state = {
				...this._state,
				messages: t.messages,
				hasMore: t.nextBefore !== void 0,
				nextBefore: t.nextBefore ?? null
			};
		} catch {} finally {
			this.disposed || this.notify();
		}
	}
	cancelConfirm() {
		this.disposed || (this._state = {
			...this._state,
			confirmingHide: !1
		}, this.notify());
	}
	showConfirmHide() {
		this.disposed || (this._state = {
			...this._state,
			confirmingHide: !0
		}, this.notify());
	}
	toggleCreate() {
		this.disposed || (this._state = {
			...this._state,
			creating: !this._state.creating
		}, this.notify());
	}
	notify() {
		this.onChange();
	}
}, { entries: g, setPrototypeOf: _, isFrozen: v, getPrototypeOf: ee, getOwnPropertyDescriptor: te } = Object, { freeze: y, seal: b, create: ne } = Object, { apply: re, construct: ie } = typeof Reflect < "u" && Reflect;
y ||= function(e) {
	return e;
}, b ||= function(e) {
	return e;
}, re ||= function(e, t, n) {
	return e.apply(t, n);
}, ie ||= function(e, t) {
	return new e(...t);
};
var ae = C(Array.prototype.forEach), oe = C(Array.prototype.lastIndexOf), se = C(Array.prototype.pop), ce = C(Array.prototype.push), le = C(Array.prototype.splice), ue = C(String.prototype.toLowerCase), de = C(String.prototype.toString), fe = C(String.prototype.match), pe = C(String.prototype.replace), me = C(String.prototype.indexOf), he = C(String.prototype.trim), x = C(Object.prototype.hasOwnProperty), S = C(RegExp.prototype.test), ge = _e(TypeError);
function C(e) {
	return function(t) {
		t instanceof RegExp && (t.lastIndex = 0);
		var n = [...arguments].slice(1);
		return re(e, t, n);
	};
}
function _e(e) {
	return function() {
		return ie(e, [...arguments]);
	};
}
function w(e, t) {
	let n = arguments.length > 2 && arguments[2] !== void 0 ? arguments[2] : ue;
	_ && _(e, null);
	let r = t.length;
	for (; r--;) {
		let i = t[r];
		if (typeof i == "string") {
			let e = n(i);
			e !== i && (v(t) || (t[r] = e), i = e);
		}
		e[i] = !0;
	}
	return e;
}
function ve(e) {
	for (let t = 0; t < e.length; t++) x(e, t) || (e[t] = null);
	return e;
}
function T(e) {
	let t = ne(null);
	for (let [n, r] of g(e)) x(e, n) && (t[n] = Array.isArray(r) ? ve(r) : r && typeof r == "object" && r.constructor === Object ? T(r) : r);
	return t;
}
function ye(e, t) {
	for (; e !== null;) {
		let n = te(e, t);
		if (n) {
			if (n.get) return C(n.get);
			if (typeof n.value == "function") return C(n.value);
		}
		e = ee(e);
	}
	function n() {
		return null;
	}
	return n;
}
var be = y(/* @__PURE__ */ "a.abbr.acronym.address.area.article.aside.audio.b.bdi.bdo.big.blink.blockquote.body.br.button.canvas.caption.center.cite.code.col.colgroup.content.data.datalist.dd.decorator.del.details.dfn.dialog.dir.div.dl.dt.element.em.fieldset.figcaption.figure.font.footer.form.h1.h2.h3.h4.h5.h6.head.header.hgroup.hr.html.i.img.input.ins.kbd.label.legend.li.main.map.mark.marquee.menu.menuitem.meter.nav.nobr.ol.optgroup.option.output.p.picture.pre.progress.q.rp.rt.ruby.s.samp.section.select.shadow.small.source.spacer.span.strike.strong.style.sub.summary.sup.table.tbody.td.template.textarea.tfoot.th.thead.time.tr.track.tt.u.ul.var.video.wbr".split(".")), xe = y(/* @__PURE__ */ "svg.a.altglyph.altglyphdef.altglyphitem.animatecolor.animatemotion.animatetransform.circle.clippath.defs.desc.ellipse.filter.font.g.glyph.glyphref.hkern.image.line.lineargradient.marker.mask.metadata.mpath.path.pattern.polygon.polyline.radialgradient.rect.stop.style.switch.symbol.text.textpath.title.tref.tspan.view.vkern".split(".")), Se = y([
	"feBlend",
	"feColorMatrix",
	"feComponentTransfer",
	"feComposite",
	"feConvolveMatrix",
	"feDiffuseLighting",
	"feDisplacementMap",
	"feDistantLight",
	"feDropShadow",
	"feFlood",
	"feFuncA",
	"feFuncB",
	"feFuncG",
	"feFuncR",
	"feGaussianBlur",
	"feImage",
	"feMerge",
	"feMergeNode",
	"feMorphology",
	"feOffset",
	"fePointLight",
	"feSpecularLighting",
	"feSpotLight",
	"feTile",
	"feTurbulence"
]), Ce = y([
	"animate",
	"color-profile",
	"cursor",
	"discard",
	"font-face",
	"font-face-format",
	"font-face-name",
	"font-face-src",
	"font-face-uri",
	"foreignobject",
	"hatch",
	"hatchpath",
	"mesh",
	"meshgradient",
	"meshpatch",
	"meshrow",
	"missing-glyph",
	"script",
	"set",
	"solidcolor",
	"unknown",
	"use"
]), we = y(/* @__PURE__ */ "math.menclose.merror.mfenced.mfrac.mglyph.mi.mlabeledtr.mmultiscripts.mn.mo.mover.mpadded.mphantom.mroot.mrow.ms.mspace.msqrt.mstyle.msub.msup.msubsup.mtable.mtd.mtext.mtr.munder.munderover.mprescripts".split(".")), Te = y([
	"maction",
	"maligngroup",
	"malignmark",
	"mlongdiv",
	"mscarries",
	"mscarry",
	"msgroup",
	"mstack",
	"msline",
	"msrow",
	"semantics",
	"annotation",
	"annotation-xml",
	"mprescripts",
	"none"
]), Ee = y(["#text"]), De = y(/* @__PURE__ */ "accept.action.align.alt.autocapitalize.autocomplete.autopictureinpicture.autoplay.background.bgcolor.border.capture.cellpadding.cellspacing.checked.cite.class.clear.color.cols.colspan.controls.controlslist.coords.crossorigin.datetime.decoding.default.dir.disabled.disablepictureinpicture.disableremoteplayback.download.draggable.enctype.enterkeyhint.face.for.headers.height.hidden.high.href.hreflang.id.inputmode.integrity.ismap.kind.label.lang.list.loading.loop.low.max.maxlength.media.method.min.minlength.multiple.muted.name.nonce.noshade.novalidate.nowrap.open.optimum.pattern.placeholder.playsinline.popover.popovertarget.popovertargetaction.poster.preload.pubdate.radiogroup.readonly.rel.required.rev.reversed.role.rows.rowspan.spellcheck.scope.selected.shape.size.sizes.span.srclang.start.src.srcset.step.style.summary.tabindex.title.translate.type.usemap.valign.value.width.wrap.xmlns.slot".split(".")), Oe = y(/* @__PURE__ */ "accent-height.accumulate.additive.alignment-baseline.amplitude.ascent.attributename.attributetype.azimuth.basefrequency.baseline-shift.begin.bias.by.class.clip.clippathunits.clip-path.clip-rule.color.color-interpolation.color-interpolation-filters.color-profile.color-rendering.cx.cy.d.dx.dy.diffuseconstant.direction.display.divisor.dur.edgemode.elevation.end.exponent.fill.fill-opacity.fill-rule.filter.filterunits.flood-color.flood-opacity.font-family.font-size.font-size-adjust.font-stretch.font-style.font-variant.font-weight.fx.fy.g1.g2.glyph-name.glyphref.gradientunits.gradienttransform.height.href.id.image-rendering.in.in2.intercept.k.k1.k2.k3.k4.kerning.keypoints.keysplines.keytimes.lang.lengthadjust.letter-spacing.kernelmatrix.kernelunitlength.lighting-color.local.marker-end.marker-mid.marker-start.markerheight.markerunits.markerwidth.maskcontentunits.maskunits.max.mask.media.method.mode.min.name.numoctaves.offset.operator.opacity.order.orient.orientation.origin.overflow.paint-order.path.pathlength.patterncontentunits.patterntransform.patternunits.points.preservealpha.preserveaspectratio.primitiveunits.r.rx.ry.radius.refx.refy.repeatcount.repeatdur.restart.result.rotate.scale.seed.shape-rendering.slope.specularconstant.specularexponent.spreadmethod.startoffset.stddeviation.stitchtiles.stop-color.stop-opacity.stroke-dasharray.stroke-dashoffset.stroke-linecap.stroke-linejoin.stroke-miterlimit.stroke-opacity.stroke.stroke-width.style.surfacescale.systemlanguage.tabindex.tablevalues.targetx.targety.transform.transform-origin.text-anchor.text-decoration.text-rendering.textlength.type.u1.u2.unicode.values.viewbox.visibility.version.vert-adv-y.vert-origin-x.vert-origin-y.width.word-spacing.wrap.writing-mode.xchannelselector.ychannelselector.x.x1.x2.xmlns.y.y1.y2.z.zoomandpan".split(".")), ke = y(/* @__PURE__ */ "accent.accentunder.align.bevelled.close.columnsalign.columnlines.columnspan.denomalign.depth.dir.display.displaystyle.encoding.fence.frame.height.href.id.largeop.length.linethickness.lspace.lquote.mathbackground.mathcolor.mathsize.mathvariant.maxsize.minsize.movablelimits.notation.numalign.open.rowalign.rowlines.rowspacing.rowspan.rspace.rquote.scriptlevel.scriptminsize.scriptsizemultiplier.selection.separator.separators.stretchy.subscriptshift.supscriptshift.symmetric.voffset.width.xmlns".split(".")), Ae = y([
	"xlink:href",
	"xml:id",
	"xlink:title",
	"xml:space",
	"xmlns:xlink"
]), je = b(/\{\{[\w\W]*|[\w\W]*\}\}/gm), E = b(/<%[\w\W]*|[\w\W]*%>/gm), Me = b(/\$\{[\w\W]*/gm), Ne = b(/^data-[\-\w.\u00B7-\uFFFF]+$/), Pe = b(/^aria-[\-\w]+$/), Fe = b(/^(?:(?:(?:f|ht)tps?|mailto|tel|callto|sms|cid|xmpp|matrix):|[^a-z]|[a-z+.\-]+(?:[^a-z+.\-:]|$))/i), Ie = b(/^(?:\w+script|data):/i), Le = b(/[\u0000-\u0020\u00A0\u1680\u180E\u2000-\u2029\u205F\u3000]/g), Re = b(/^html$/i), ze = b(/^[a-z][.\w]*(-[.\w]+)+$/i), Be = /*#__PURE__*/ Object.freeze({
	__proto__: null,
	ARIA_ATTR: Pe,
	ATTR_WHITESPACE: Le,
	CUSTOM_ELEMENT: ze,
	DATA_ATTR: Ne,
	DOCTYPE_NAME: Re,
	ERB_EXPR: E,
	IS_ALLOWED_URI: Fe,
	IS_SCRIPT_OR_DATA: Ie,
	MUSTACHE_EXPR: je,
	TMPLIT_EXPR: Me
}), Ve = {
	element: 1,
	attribute: 2,
	text: 3,
	cdataSection: 4,
	entityReference: 5,
	entityNode: 6,
	progressingInstruction: 7,
	comment: 8,
	document: 9,
	documentType: 10,
	documentFragment: 11,
	notation: 12
}, He = function() {
	return typeof window > "u" ? null : window;
}, Ue = function(e, t) {
	if (typeof e != "object" || typeof e.createPolicy != "function") return null;
	let n = null, r = "data-tt-policy-suffix";
	t && t.hasAttribute(r) && (n = t.getAttribute(r));
	let i = "dompurify" + (n ? "#" + n : "");
	try {
		return e.createPolicy(i, {
			createHTML(e) {
				return e;
			},
			createScriptURL(e) {
				return e;
			}
		});
	} catch {
		return console.warn("TrustedTypes policy " + i + " could not be created."), null;
	}
}, We = function() {
	return {
		afterSanitizeAttributes: [],
		afterSanitizeElements: [],
		afterSanitizeShadowDOM: [],
		beforeSanitizeAttributes: [],
		beforeSanitizeElements: [],
		beforeSanitizeShadowDOM: [],
		uponSanitizeAttribute: [],
		uponSanitizeElement: [],
		uponSanitizeShadowNode: []
	};
};
function Ge() {
	let e = arguments.length > 0 && arguments[0] !== void 0 ? arguments[0] : He(), t = (e) => Ge(e);
	if (t.version = "3.2.6", t.removed = [], !e || !e.document || e.document.nodeType !== Ve.document || !e.Element) return t.isSupported = !1, t;
	let { document: n } = e, r = n, i = r.currentScript, { DocumentFragment: a, HTMLTemplateElement: o, Node: s, Element: c, NodeFilter: l, NamedNodeMap: u = e.NamedNodeMap || e.MozNamedAttrMap, HTMLFormElement: d, DOMParser: f, trustedTypes: p } = e, m = c.prototype, h = ye(m, "cloneNode"), _ = ye(m, "remove"), v = ye(m, "nextSibling"), ee = ye(m, "childNodes"), te = ye(m, "parentNode");
	if (typeof o == "function") {
		let e = n.createElement("template");
		e.content && e.content.ownerDocument && (n = e.content.ownerDocument);
	}
	let b, re = "", { implementation: ie, createNodeIterator: C, createDocumentFragment: _e, getElementsByTagName: ve } = n, { importNode: je } = r, E = We();
	t.isSupported = typeof g == "function" && typeof te == "function" && ie && ie.createHTMLDocument !== void 0;
	let { MUSTACHE_EXPR: Me, ERB_EXPR: Ne, TMPLIT_EXPR: Pe, DATA_ATTR: Ie, ARIA_ATTR: Le, IS_SCRIPT_OR_DATA: ze, ATTR_WHITESPACE: Ke, CUSTOM_ELEMENT: qe } = Be, { IS_ALLOWED_URI: Je } = Be, D = null, Ye = w({}, [
		...be,
		...xe,
		...Se,
		...we,
		...Ee
	]), O = null, Xe = w({}, [
		...De,
		...Oe,
		...ke,
		...Ae
	]), k = Object.seal(ne(null, {
		tagNameCheck: {
			writable: !0,
			configurable: !1,
			enumerable: !0,
			value: null
		},
		attributeNameCheck: {
			writable: !0,
			configurable: !1,
			enumerable: !0,
			value: null
		},
		allowCustomizedBuiltInElements: {
			writable: !0,
			configurable: !1,
			enumerable: !0,
			value: !1
		}
	})), A = null, Ze = null, Qe = !0, $e = !0, et = !1, tt = !0, nt = !1, rt = !0, j = !1, it = !1, at = !1, M = !1, ot = !1, st = !1, ct = !0, lt = !1, ut = !0, dt = !1, ft = {}, pt = null, mt = w({}, [
		"annotation-xml",
		"audio",
		"colgroup",
		"desc",
		"foreignobject",
		"head",
		"iframe",
		"math",
		"mi",
		"mn",
		"mo",
		"ms",
		"mtext",
		"noembed",
		"noframes",
		"noscript",
		"plaintext",
		"script",
		"style",
		"svg",
		"template",
		"thead",
		"title",
		"video",
		"xmp"
	]), ht = null, gt = w({}, [
		"audio",
		"video",
		"img",
		"source",
		"image",
		"track"
	]), _t = null, vt = w({}, [
		"alt",
		"class",
		"for",
		"id",
		"label",
		"name",
		"pattern",
		"placeholder",
		"role",
		"summary",
		"title",
		"value",
		"style",
		"xmlns"
	]), N = "http://www.w3.org/1998/Math/MathML", yt = "http://www.w3.org/2000/svg", P = "http://www.w3.org/1999/xhtml", F = P, bt = !1, xt = null, St = w({}, [
		N,
		yt,
		P
	], de), I = w({}, [
		"mi",
		"mo",
		"mn",
		"ms",
		"mtext"
	]), L = w({}, ["annotation-xml"]), Ct = w({}, [
		"title",
		"style",
		"font",
		"a",
		"script"
	]), wt = null, Tt = ["application/xhtml+xml", "text/html"], R = null, z = null, Et = n.createElement("form"), Dt = function(e) {
		return e instanceof RegExp || e instanceof Function;
	}, Ot = function() {
		let e = arguments.length > 0 && arguments[0] !== void 0 ? arguments[0] : {};
		if (!(z && z === e)) {
			if ((!e || typeof e != "object") && (e = {}), e = T(e), wt = Tt.indexOf(e.PARSER_MEDIA_TYPE) === -1 ? "text/html" : e.PARSER_MEDIA_TYPE, R = wt === "application/xhtml+xml" ? de : ue, D = x(e, "ALLOWED_TAGS") ? w({}, e.ALLOWED_TAGS, R) : Ye, O = x(e, "ALLOWED_ATTR") ? w({}, e.ALLOWED_ATTR, R) : Xe, xt = x(e, "ALLOWED_NAMESPACES") ? w({}, e.ALLOWED_NAMESPACES, de) : St, _t = x(e, "ADD_URI_SAFE_ATTR") ? w(T(vt), e.ADD_URI_SAFE_ATTR, R) : vt, ht = x(e, "ADD_DATA_URI_TAGS") ? w(T(gt), e.ADD_DATA_URI_TAGS, R) : gt, pt = x(e, "FORBID_CONTENTS") ? w({}, e.FORBID_CONTENTS, R) : mt, A = x(e, "FORBID_TAGS") ? w({}, e.FORBID_TAGS, R) : T({}), Ze = x(e, "FORBID_ATTR") ? w({}, e.FORBID_ATTR, R) : T({}), ft = x(e, "USE_PROFILES") ? e.USE_PROFILES : !1, Qe = e.ALLOW_ARIA_ATTR !== !1, $e = e.ALLOW_DATA_ATTR !== !1, et = e.ALLOW_UNKNOWN_PROTOCOLS || !1, tt = e.ALLOW_SELF_CLOSE_IN_ATTR !== !1, nt = e.SAFE_FOR_TEMPLATES || !1, rt = e.SAFE_FOR_XML !== !1, j = e.WHOLE_DOCUMENT || !1, M = e.RETURN_DOM || !1, ot = e.RETURN_DOM_FRAGMENT || !1, st = e.RETURN_TRUSTED_TYPE || !1, at = e.FORCE_BODY || !1, ct = e.SANITIZE_DOM !== !1, lt = e.SANITIZE_NAMED_PROPS || !1, ut = e.KEEP_CONTENT !== !1, dt = e.IN_PLACE || !1, Je = e.ALLOWED_URI_REGEXP || Fe, F = e.NAMESPACE || P, I = e.MATHML_TEXT_INTEGRATION_POINTS || I, L = e.HTML_INTEGRATION_POINTS || L, k = e.CUSTOM_ELEMENT_HANDLING || {}, e.CUSTOM_ELEMENT_HANDLING && Dt(e.CUSTOM_ELEMENT_HANDLING.tagNameCheck) && (k.tagNameCheck = e.CUSTOM_ELEMENT_HANDLING.tagNameCheck), e.CUSTOM_ELEMENT_HANDLING && Dt(e.CUSTOM_ELEMENT_HANDLING.attributeNameCheck) && (k.attributeNameCheck = e.CUSTOM_ELEMENT_HANDLING.attributeNameCheck), e.CUSTOM_ELEMENT_HANDLING && typeof e.CUSTOM_ELEMENT_HANDLING.allowCustomizedBuiltInElements == "boolean" && (k.allowCustomizedBuiltInElements = e.CUSTOM_ELEMENT_HANDLING.allowCustomizedBuiltInElements), nt && ($e = !1), ot && (M = !0), ft && (D = w({}, Ee), O = [], ft.html === !0 && (w(D, be), w(O, De)), ft.svg === !0 && (w(D, xe), w(O, Oe), w(O, Ae)), ft.svgFilters === !0 && (w(D, Se), w(O, Oe), w(O, Ae)), ft.mathMl === !0 && (w(D, we), w(O, ke), w(O, Ae))), e.ADD_TAGS && (D === Ye && (D = T(D)), w(D, e.ADD_TAGS, R)), e.ADD_ATTR && (O === Xe && (O = T(O)), w(O, e.ADD_ATTR, R)), e.ADD_URI_SAFE_ATTR && w(_t, e.ADD_URI_SAFE_ATTR, R), e.FORBID_CONTENTS && (pt === mt && (pt = T(pt)), w(pt, e.FORBID_CONTENTS, R)), ut && (D["#text"] = !0), j && w(D, [
				"html",
				"head",
				"body"
			]), D.table && (w(D, ["tbody"]), delete A.tbody), e.TRUSTED_TYPES_POLICY) {
				if (typeof e.TRUSTED_TYPES_POLICY.createHTML != "function") throw ge("TRUSTED_TYPES_POLICY configuration option must provide a \"createHTML\" hook.");
				if (typeof e.TRUSTED_TYPES_POLICY.createScriptURL != "function") throw ge("TRUSTED_TYPES_POLICY configuration option must provide a \"createScriptURL\" hook.");
				b = e.TRUSTED_TYPES_POLICY, re = b.createHTML("");
			} else b === void 0 && (b = Ue(p, i)), b !== null && typeof re == "string" && (re = b.createHTML(""));
			y && y(e), z = e;
		}
	}, kt = w({}, [
		...xe,
		...Se,
		...Ce
	]), At = w({}, [...we, ...Te]), jt = function(e) {
		let t = te(e);
		(!t || !t.tagName) && (t = {
			namespaceURI: F,
			tagName: "template"
		});
		let n = ue(e.tagName), r = ue(t.tagName);
		return xt[e.namespaceURI] ? e.namespaceURI === yt ? t.namespaceURI === P ? n === "svg" : t.namespaceURI === N ? n === "svg" && (r === "annotation-xml" || I[r]) : !!kt[n] : e.namespaceURI === N ? t.namespaceURI === P ? n === "math" : t.namespaceURI === yt ? n === "math" && L[r] : !!At[n] : e.namespaceURI === P ? t.namespaceURI === yt && !L[r] || t.namespaceURI === N && !I[r] ? !1 : !At[n] && (Ct[n] || !kt[n]) : !!(wt === "application/xhtml+xml" && xt[e.namespaceURI]) : !1;
	}, B = function(e) {
		ce(t.removed, { element: e });
		try {
			te(e).removeChild(e);
		} catch {
			_(e);
		}
	}, V = function(e, n) {
		try {
			ce(t.removed, {
				attribute: n.getAttributeNode(e),
				from: n
			});
		} catch {
			ce(t.removed, {
				attribute: null,
				from: n
			});
		}
		if (n.removeAttribute(e), e === "is") {
			if (M || ot) try {
				B(n);
			} catch {}
			else try {
				n.setAttribute(e, "");
			} catch {}
		}
	}, Mt = function(e) {
		let t = null, r = null;
		if (at) e = "<remove></remove>" + e;
		else {
			let t = fe(e, /^[\r\n\t ]+/);
			r = t && t[0];
		}
		wt === "application/xhtml+xml" && F === P && (e = "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head></head><body>" + e + "</body></html>");
		let i = b ? b.createHTML(e) : e;
		if (F === P) try {
			t = new f().parseFromString(i, wt);
		} catch {}
		if (!t || !t.documentElement) {
			t = ie.createDocument(F, "template", null);
			try {
				t.documentElement.innerHTML = bt ? re : i;
			} catch {}
		}
		let a = t.body || t.documentElement;
		return e && r && a.insertBefore(n.createTextNode(r), a.childNodes[0] || null), F === P ? ve.call(t, j ? "html" : "body")[0] : j ? t.documentElement : a;
	}, Nt = function(e) {
		return C.call(e.ownerDocument || e, e, l.SHOW_ELEMENT | l.SHOW_COMMENT | l.SHOW_TEXT | l.SHOW_PROCESSING_INSTRUCTION | l.SHOW_CDATA_SECTION, null);
	}, Pt = function(e) {
		return e instanceof d && (typeof e.nodeName != "string" || typeof e.textContent != "string" || typeof e.removeChild != "function" || !(e.attributes instanceof u) || typeof e.removeAttribute != "function" || typeof e.setAttribute != "function" || typeof e.namespaceURI != "string" || typeof e.insertBefore != "function" || typeof e.hasChildNodes != "function");
	}, Ft = function(e) {
		return typeof s == "function" && e instanceof s;
	};
	function H(e, n, r) {
		ae(e, (e) => {
			e.call(t, n, r, z);
		});
	}
	let It = function(e) {
		let n = null;
		if (H(E.beforeSanitizeElements, e, null), Pt(e)) return B(e), !0;
		let r = R(e.nodeName);
		if (H(E.uponSanitizeElement, e, {
			tagName: r,
			allowedTags: D
		}), rt && e.hasChildNodes() && !Ft(e.firstElementChild) && S(/<[/\w!]/g, e.innerHTML) && S(/<[/\w!]/g, e.textContent) || e.nodeType === Ve.progressingInstruction || rt && e.nodeType === Ve.comment && S(/<[/\w]/g, e.data)) return B(e), !0;
		if (!D[r] || A[r]) {
			if (!A[r] && Lt(r) && (k.tagNameCheck instanceof RegExp && S(k.tagNameCheck, r) || k.tagNameCheck instanceof Function && k.tagNameCheck(r))) return !1;
			if (ut && !pt[r]) {
				let t = te(e) || e.parentNode, n = ee(e) || e.childNodes;
				if (n && t) {
					let r = n.length;
					for (let i = r - 1; i >= 0; --i) {
						let r = h(n[i], !0);
						r.__removalCount = (e.__removalCount || 0) + 1, t.insertBefore(r, v(e));
					}
				}
			}
			return B(e), !0;
		}
		return e instanceof c && !jt(e) || (r === "noscript" || r === "noembed" || r === "noframes") && S(/<\/no(script|embed|frames)/i, e.innerHTML) ? (B(e), !0) : (nt && e.nodeType === Ve.text && (n = e.textContent, ae([
			Me,
			Ne,
			Pe
		], (e) => {
			n = pe(n, e, " ");
		}), e.textContent !== n && (ce(t.removed, { element: e.cloneNode() }), e.textContent = n)), H(E.afterSanitizeElements, e, null), !1);
	}, U = function(e, t, r) {
		if (ct && (t === "id" || t === "name") && (r in n || r in Et)) return !1;
		if (!($e && !Ze[t] && S(Ie, t)) && !(Qe && S(Le, t))) {
			if (!O[t] || Ze[t]) {
				if (!(Lt(e) && (k.tagNameCheck instanceof RegExp && S(k.tagNameCheck, e) || k.tagNameCheck instanceof Function && k.tagNameCheck(e)) && (k.attributeNameCheck instanceof RegExp && S(k.attributeNameCheck, t) || k.attributeNameCheck instanceof Function && k.attributeNameCheck(t)) || t === "is" && k.allowCustomizedBuiltInElements && (k.tagNameCheck instanceof RegExp && S(k.tagNameCheck, r) || k.tagNameCheck instanceof Function && k.tagNameCheck(r)))) return !1;
			} else if (!_t[t] && !S(Je, pe(r, Ke, "")) && (t !== "src" && t !== "xlink:href" && t !== "href" || e === "script" || me(r, "data:") !== 0 || !ht[e]) && (!et || S(ze, pe(r, Ke, ""))) && r) return !1;
		}
		return !0;
	}, Lt = function(e) {
		return e !== "annotation-xml" && fe(e, qe);
	}, Rt = function(e) {
		H(E.beforeSanitizeAttributes, e, null);
		let { attributes: n } = e;
		if (!n || Pt(e)) return;
		let r = {
			attrName: "",
			attrValue: "",
			keepAttr: !0,
			allowedAttributes: O,
			forceKeepAttr: void 0
		}, i = n.length;
		for (; i--;) {
			let { name: a, namespaceURI: o, value: s } = n[i], c = R(a), l = s, u = a === "value" ? l : he(l);
			if (r.attrName = c, r.attrValue = u, r.keepAttr = !0, r.forceKeepAttr = void 0, H(E.uponSanitizeAttribute, e, r), u = r.attrValue, lt && (c === "id" || c === "name") && (V(a, e), u = "user-content-" + u), rt && S(/((--!?|])>)|<\/(style|title)/i, u)) {
				V(a, e);
				continue;
			}
			if (r.forceKeepAttr) continue;
			if (!r.keepAttr) {
				V(a, e);
				continue;
			}
			if (!tt && S(/\/>/i, u)) {
				V(a, e);
				continue;
			}
			nt && ae([
				Me,
				Ne,
				Pe
			], (e) => {
				u = pe(u, e, " ");
			});
			let d = R(e.nodeName);
			if (!U(d, c, u)) {
				V(a, e);
				continue;
			}
			if (b && typeof p == "object" && typeof p.getAttributeType == "function" && !o) switch (p.getAttributeType(d, c)) {
				case "TrustedHTML":
					u = b.createHTML(u);
					break;
				case "TrustedScriptURL": u = b.createScriptURL(u);
			}
			if (u !== l) try {
				o ? e.setAttributeNS(o, a, u) : e.setAttribute(a, u), Pt(e) ? B(e) : se(t.removed);
			} catch {
				V(a, e);
			}
		}
		H(E.afterSanitizeAttributes, e, null);
	}, zt = function e(t) {
		let n = null, r = Nt(t);
		for (H(E.beforeSanitizeShadowDOM, t, null); n = r.nextNode();) H(E.uponSanitizeShadowNode, n, null), It(n), Rt(n), n.content instanceof a && e(n.content);
		H(E.afterSanitizeShadowDOM, t, null);
	};
	return t.sanitize = function(e) {
		let n = arguments.length > 1 && arguments[1] !== void 0 ? arguments[1] : {}, i = null, o = null, c = null, l = null;
		if (bt = !e, bt && (e = "<!-->"), typeof e != "string" && !Ft(e)) {
			if (typeof e.toString == "function") {
				if (e = e.toString(), typeof e != "string") throw ge("dirty is not a string, aborting");
			} else throw ge("toString is not a function");
		}
		if (!t.isSupported) return e;
		if (it || Ot(n), t.removed = [], typeof e == "string" && (dt = !1), dt) {
			if (e.nodeName) {
				let t = R(e.nodeName);
				if (!D[t] || A[t]) throw ge("root node is forbidden and cannot be sanitized in-place");
			}
		} else if (e instanceof s) i = Mt("<!---->"), o = i.ownerDocument.importNode(e, !0), o.nodeType === Ve.element && o.nodeName === "BODY" || o.nodeName === "HTML" ? i = o : i.appendChild(o);
		else {
			if (!M && !nt && !j && e.indexOf("<") === -1) return b && st ? b.createHTML(e) : e;
			if (i = Mt(e), !i) return M ? null : st ? re : "";
		}
		i && at && B(i.firstChild);
		let u = Nt(dt ? e : i);
		for (; c = u.nextNode();) It(c), Rt(c), c.content instanceof a && zt(c.content);
		if (dt) return e;
		if (M) {
			if (ot) for (l = _e.call(i.ownerDocument); i.firstChild;) l.appendChild(i.firstChild);
			else l = i;
			return (O.shadowroot || O.shadowrootmode) && (l = je.call(r, l, !0)), l;
		}
		let d = j ? i.outerHTML : i.innerHTML;
		return j && D["!doctype"] && i.ownerDocument && i.ownerDocument.doctype && i.ownerDocument.doctype.name && S(Re, i.ownerDocument.doctype.name) && (d = "<!DOCTYPE " + i.ownerDocument.doctype.name + ">\n" + d), nt && ae([
			Me,
			Ne,
			Pe
		], (e) => {
			d = pe(d, e, " ");
		}), b && st ? b.createHTML(d) : d;
	}, t.setConfig = function() {
		let e = arguments.length > 0 && arguments[0] !== void 0 ? arguments[0] : {};
		Ot(e), it = !0;
	}, t.clearConfig = function() {
		z = null, it = !1;
	}, t.isValidAttribute = function(e, t, n) {
		z || Ot({});
		let r = R(e), i = R(t);
		return U(r, i, n);
	}, t.addHook = function(e, t) {
		typeof t == "function" && ce(E[e], t);
	}, t.removeHook = function(e, t) {
		if (t !== void 0) {
			let n = oe(E[e], t);
			return n === -1 ? void 0 : le(E[e], n, 1)[0];
		}
		return se(E[e]);
	}, t.removeHooks = function(e) {
		E[e] = [];
	}, t.removeAllHooks = function() {
		E = We();
	}, t;
}
var Ke = Ge(), qe = {};
function Je(e) {
	let t = qe[e];
	if (t) return t;
	t = qe[e] = [];
	for (let e = 0; e < 128; e++) {
		let n = String.fromCharCode(e);
		t.push(n);
	}
	for (let n = 0; n < e.length; n++) {
		let r = e.charCodeAt(n);
		t[r] = "%" + ("0" + r.toString(16).toUpperCase()).slice(-2);
	}
	return t;
}
function D(e, t) {
	typeof t != "string" && (t = D.defaultChars);
	let n = Je(t);
	return e.replace(/(%[a-f0-9]{2})+/gi, function(e) {
		let t = "";
		for (let r = 0, i = e.length; r < i; r += 3) {
			let a = parseInt(e.slice(r + 1, r + 3), 16);
			if (a < 128) {
				t += n[a];
				continue;
			}
			if ((a & 224) == 192 && r + 3 < i) {
				let n = parseInt(e.slice(r + 4, r + 6), 16);
				if ((n & 192) == 128) {
					let e = a << 6 & 1984 | n & 63;
					t += e < 128 ? "��" : String.fromCharCode(e), r += 3;
					continue;
				}
			}
			if ((a & 240) == 224 && r + 6 < i) {
				let n = parseInt(e.slice(r + 4, r + 6), 16), i = parseInt(e.slice(r + 7, r + 9), 16);
				if ((n & 192) == 128 && (i & 192) == 128) {
					let e = a << 12 & 61440 | n << 6 & 4032 | i & 63;
					t += e < 2048 || e >= 55296 && e <= 57343 ? "���" : String.fromCharCode(e), r += 6;
					continue;
				}
			}
			if ((a & 248) == 240 && r + 9 < i) {
				let n = parseInt(e.slice(r + 4, r + 6), 16), i = parseInt(e.slice(r + 7, r + 9), 16), o = parseInt(e.slice(r + 10, r + 12), 16);
				if ((n & 192) == 128 && (i & 192) == 128 && (o & 192) == 128) {
					let e = a << 18 & 1835008 | n << 12 & 258048 | i << 6 & 4032 | o & 63;
					e < 65536 || e > 1114111 ? t += "����" : (e -= 65536, t += String.fromCharCode(55296 + (e >> 10), 56320 + (e & 1023))), r += 9;
					continue;
				}
			}
			t += "�";
		}
		return t;
	});
}
D.defaultChars = ";/?:@&=+$,#", D.componentChars = "";
//#endregion
//#region ../../../node_modules/mdurl/lib/encode.mjs
var Ye = {};
function O(e) {
	let t = Ye[e];
	if (t) return t;
	t = Ye[e] = [];
	for (let e = 0; e < 128; e++) {
		let n = String.fromCharCode(e);
		/^[0-9a-z]$/i.test(n) ? t.push(n) : t.push("%" + ("0" + e.toString(16).toUpperCase()).slice(-2));
	}
	for (let n = 0; n < e.length; n++) t[e.charCodeAt(n)] = e[n];
	return t;
}
function Xe(e, t, n) {
	typeof t != "string" && (n = t, t = Xe.defaultChars), n === void 0 && (n = !0);
	let r = O(t), i = "";
	for (let t = 0, a = e.length; t < a; t++) {
		let o = e.charCodeAt(t);
		if (n && o === 37 && t + 2 < a && /^[0-9a-f]{2}$/i.test(e.slice(t + 1, t + 3))) {
			i += e.slice(t, t + 3), t += 2;
			continue;
		}
		if (o < 128) {
			i += r[o];
			continue;
		}
		if (o >= 55296 && o <= 57343) {
			if (o >= 55296 && o <= 56319 && t + 1 < a) {
				let n = e.charCodeAt(t + 1);
				if (n >= 56320 && n <= 57343) {
					i += encodeURIComponent(e[t] + e[t + 1]), t++;
					continue;
				}
			}
			i += "%EF%BF%BD";
			continue;
		}
		i += encodeURIComponent(e[t]);
	}
	return i;
}
Xe.defaultChars = ";/?:@&=+$,-_.!~*'()#", Xe.componentChars = "-_.!~*'()";
//#endregion
//#region ../../../node_modules/mdurl/lib/format.mjs
function k(e) {
	let t = "";
	return t += e.protocol || "", t += e.slashes ? "//" : "", t += e.auth ? e.auth + "@" : "", e.hostname && e.hostname.indexOf(":") !== -1 ? t += "[" + e.hostname + "]" : t += e.hostname || "", t += e.port ? ":" + e.port : "", t += e.pathname || "", t += e.search || "", t += e.hash || "", t;
}
//#endregion
//#region ../../../node_modules/mdurl/lib/parse.mjs
function A() {
	this.protocol = null, this.slashes = null, this.auth = null, this.port = null, this.hostname = null, this.hash = null, this.search = null, this.pathname = null;
}
var Ze = /^([a-z0-9.+-]+:)/i, Qe = /:[0-9]*$/, $e = /^(\/\/?(?!\/)[^\?\s]*)(\?[^\s]*)?$/, et = [
	"%",
	"/",
	"?",
	";",
	"#",
	"'",
	"{",
	"}",
	"|",
	"\\",
	"^",
	"`",
	"<",
	">",
	"\"",
	"`",
	" ",
	"\r",
	"\n",
	"	"
], tt = [
	"/",
	"?",
	"#"
], nt = 255, rt = /^[+a-z0-9A-Z_-]{0,63}$/, j = /^([+a-z0-9A-Z_-]{0,63})(.*)$/, it = {
	javascript: !0,
	"javascript:": !0
}, at = {
	http: !0,
	https: !0,
	ftp: !0,
	gopher: !0,
	file: !0,
	"http:": !0,
	"https:": !0,
	"ftp:": !0,
	"gopher:": !0,
	"file:": !0
};
function M(e, t) {
	if (e && e instanceof A) return e;
	let n = new A();
	return n.parse(e, t), n;
}
A.prototype.parse = function(e, t) {
	let n, r, i, a = e;
	if (a = a.trim(), !t && e.split("#").length === 1) {
		let e = $e.exec(a);
		if (e) return this.pathname = e[1], e[2] && (this.search = e[2]), this;
	}
	let o = Ze.exec(a);
	if (o && (o = o[0], n = o.toLowerCase(), this.protocol = o, a = a.substr(o.length)), (t || o || a.match(/^\/\/[^@\/]+@[^@\/]+/)) && (i = a.substr(0, 2) === "//", i && !(o && it[o]) && (a = a.substr(2), this.slashes = !0)), !it[o] && (i || o && !at[o])) {
		let e = -1;
		for (let t = 0; t < tt.length; t++) r = a.indexOf(tt[t]), r !== -1 && (e === -1 || r < e) && (e = r);
		let t, n;
		n = e === -1 ? a.lastIndexOf("@") : a.lastIndexOf("@", e), n !== -1 && (t = a.slice(0, n), a = a.slice(n + 1), this.auth = t), e = -1;
		for (let t = 0; t < et.length; t++) r = a.indexOf(et[t]), r !== -1 && (e === -1 || r < e) && (e = r);
		e === -1 && (e = a.length), a[e - 1] === ":" && e--;
		let i = a.slice(0, e);
		a = a.slice(e), this.parseHost(i), this.hostname = this.hostname || "";
		let o = this.hostname[0] === "[" && this.hostname[this.hostname.length - 1] === "]";
		if (!o) {
			let e = this.hostname.split(/\./);
			for (let t = 0, n = e.length; t < n; t++) {
				let n = e[t];
				if (n && !n.match(rt)) {
					let r = "";
					for (let e = 0, t = n.length; e < t; e++) n.charCodeAt(e) > 127 ? r += "x" : r += n[e];
					if (!r.match(rt)) {
						let r = e.slice(0, t), i = e.slice(t + 1), o = n.match(j);
						o && (r.push(o[1]), i.unshift(o[2])), i.length && (a = i.join(".") + a), this.hostname = r.join(".");
						break;
					}
				}
			}
		}
		this.hostname.length > nt && (this.hostname = ""), o && (this.hostname = this.hostname.substr(1, this.hostname.length - 2));
	}
	let s = a.indexOf("#");
	s !== -1 && (this.hash = a.substr(s), a = a.slice(0, s));
	let c = a.indexOf("?");
	return c !== -1 && (this.search = a.substr(c), a = a.slice(0, c)), a && (this.pathname = a), at[n] && this.hostname && !this.pathname && (this.pathname = ""), this;
}, A.prototype.parseHost = function(e) {
	let t = Qe.exec(e);
	t && (t = t[0], t !== ":" && (this.port = t.substr(1)), e = e.substr(0, e.length - t.length)), e && (this.hostname = e);
};
//#endregion
//#region ../../../node_modules/mdurl/index.mjs
var ot = /* @__PURE__ */ t({
	decode: () => D,
	encode: () => Xe,
	format: () => k,
	parse: () => M
}), st = /[\0-\uD7FF\uE000-\uFFFF]|[\uD800-\uDBFF][\uDC00-\uDFFF]|[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?:[^\uD800-\uDBFF]|^)[\uDC00-\uDFFF]/, ct = /[\0-\x1F\x7F-\x9F]/, lt = /[\xAD\u0600-\u0605\u061C\u06DD\u070F\u0890\u0891\u08E2\u180E\u200B-\u200F\u202A-\u202E\u2060-\u2064\u2066-\u206F\uFEFF\uFFF9-\uFFFB]|\uD804[\uDCBD\uDCCD]|\uD80D[\uDC30-\uDC3F]|\uD82F[\uDCA0-\uDCA3]|\uD834[\uDD73-\uDD7A]|\uDB40[\uDC01\uDC20-\uDC7F]/, ut = /[!-#%-\*,-\/:;\?@\[-\]_\{\}\xA1\xA7\xAB\xB6\xB7\xBB\xBF\u037E\u0387\u055A-\u055F\u0589\u058A\u05BE\u05C0\u05C3\u05C6\u05F3\u05F4\u0609\u060A\u060C\u060D\u061B\u061D-\u061F\u066A-\u066D\u06D4\u0700-\u070D\u07F7-\u07F9\u0830-\u083E\u085E\u0964\u0965\u0970\u09FD\u0A76\u0AF0\u0C77\u0C84\u0DF4\u0E4F\u0E5A\u0E5B\u0F04-\u0F12\u0F14\u0F3A-\u0F3D\u0F85\u0FD0-\u0FD4\u0FD9\u0FDA\u104A-\u104F\u10FB\u1360-\u1368\u1400\u166E\u169B\u169C\u16EB-\u16ED\u1735\u1736\u17D4-\u17D6\u17D8-\u17DA\u1800-\u180A\u1944\u1945\u1A1E\u1A1F\u1AA0-\u1AA6\u1AA8-\u1AAD\u1B5A-\u1B60\u1B7D\u1B7E\u1BFC-\u1BFF\u1C3B-\u1C3F\u1C7E\u1C7F\u1CC0-\u1CC7\u1CD3\u2010-\u2027\u2030-\u2043\u2045-\u2051\u2053-\u205E\u207D\u207E\u208D\u208E\u2308-\u230B\u2329\u232A\u2768-\u2775\u27C5\u27C6\u27E6-\u27EF\u2983-\u2998\u29D8-\u29DB\u29FC\u29FD\u2CF9-\u2CFC\u2CFE\u2CFF\u2D70\u2E00-\u2E2E\u2E30-\u2E4F\u2E52-\u2E5D\u3001-\u3003\u3008-\u3011\u3014-\u301F\u3030\u303D\u30A0\u30FB\uA4FE\uA4FF\uA60D-\uA60F\uA673\uA67E\uA6F2-\uA6F7\uA874-\uA877\uA8CE\uA8CF\uA8F8-\uA8FA\uA8FC\uA92E\uA92F\uA95F\uA9C1-\uA9CD\uA9DE\uA9DF\uAA5C-\uAA5F\uAADE\uAADF\uAAF0\uAAF1\uABEB\uFD3E\uFD3F\uFE10-\uFE19\uFE30-\uFE52\uFE54-\uFE61\uFE63\uFE68\uFE6A\uFE6B\uFF01-\uFF03\uFF05-\uFF0A\uFF0C-\uFF0F\uFF1A\uFF1B\uFF1F\uFF20\uFF3B-\uFF3D\uFF3F\uFF5B\uFF5D\uFF5F-\uFF65]|\uD800[\uDD00-\uDD02\uDF9F\uDFD0]|\uD801\uDD6F|\uD802[\uDC57\uDD1F\uDD3F\uDE50-\uDE58\uDE7F\uDEF0-\uDEF6\uDF39-\uDF3F\uDF99-\uDF9C]|\uD803[\uDEAD\uDF55-\uDF59\uDF86-\uDF89]|\uD804[\uDC47-\uDC4D\uDCBB\uDCBC\uDCBE-\uDCC1\uDD40-\uDD43\uDD74\uDD75\uDDC5-\uDDC8\uDDCD\uDDDB\uDDDD-\uDDDF\uDE38-\uDE3D\uDEA9]|\uD805[\uDC4B-\uDC4F\uDC5A\uDC5B\uDC5D\uDCC6\uDDC1-\uDDD7\uDE41-\uDE43\uDE60-\uDE6C\uDEB9\uDF3C-\uDF3E]|\uD806[\uDC3B\uDD44-\uDD46\uDDE2\uDE3F-\uDE46\uDE9A-\uDE9C\uDE9E-\uDEA2\uDF00-\uDF09]|\uD807[\uDC41-\uDC45\uDC70\uDC71\uDEF7\uDEF8\uDF43-\uDF4F\uDFFF]|\uD809[\uDC70-\uDC74]|\uD80B[\uDFF1\uDFF2]|\uD81A[\uDE6E\uDE6F\uDEF5\uDF37-\uDF3B\uDF44]|\uD81B[\uDE97-\uDE9A\uDFE2]|\uD82F\uDC9F|\uD836[\uDE87-\uDE8B]|\uD83A[\uDD5E\uDD5F]/, dt = /[\$\+<->\^`\|~\xA2-\xA6\xA8\xA9\xAC\xAE-\xB1\xB4\xB8\xD7\xF7\u02C2-\u02C5\u02D2-\u02DF\u02E5-\u02EB\u02ED\u02EF-\u02FF\u0375\u0384\u0385\u03F6\u0482\u058D-\u058F\u0606-\u0608\u060B\u060E\u060F\u06DE\u06E9\u06FD\u06FE\u07F6\u07FE\u07FF\u0888\u09F2\u09F3\u09FA\u09FB\u0AF1\u0B70\u0BF3-\u0BFA\u0C7F\u0D4F\u0D79\u0E3F\u0F01-\u0F03\u0F13\u0F15-\u0F17\u0F1A-\u0F1F\u0F34\u0F36\u0F38\u0FBE-\u0FC5\u0FC7-\u0FCC\u0FCE\u0FCF\u0FD5-\u0FD8\u109E\u109F\u1390-\u1399\u166D\u17DB\u1940\u19DE-\u19FF\u1B61-\u1B6A\u1B74-\u1B7C\u1FBD\u1FBF-\u1FC1\u1FCD-\u1FCF\u1FDD-\u1FDF\u1FED-\u1FEF\u1FFD\u1FFE\u2044\u2052\u207A-\u207C\u208A-\u208C\u20A0-\u20C0\u2100\u2101\u2103-\u2106\u2108\u2109\u2114\u2116-\u2118\u211E-\u2123\u2125\u2127\u2129\u212E\u213A\u213B\u2140-\u2144\u214A-\u214D\u214F\u218A\u218B\u2190-\u2307\u230C-\u2328\u232B-\u2426\u2440-\u244A\u249C-\u24E9\u2500-\u2767\u2794-\u27C4\u27C7-\u27E5\u27F0-\u2982\u2999-\u29D7\u29DC-\u29FB\u29FE-\u2B73\u2B76-\u2B95\u2B97-\u2BFF\u2CE5-\u2CEA\u2E50\u2E51\u2E80-\u2E99\u2E9B-\u2EF3\u2F00-\u2FD5\u2FF0-\u2FFF\u3004\u3012\u3013\u3020\u3036\u3037\u303E\u303F\u309B\u309C\u3190\u3191\u3196-\u319F\u31C0-\u31E3\u31EF\u3200-\u321E\u322A-\u3247\u3250\u3260-\u327F\u328A-\u32B0\u32C0-\u33FF\u4DC0-\u4DFF\uA490-\uA4C6\uA700-\uA716\uA720\uA721\uA789\uA78A\uA828-\uA82B\uA836-\uA839\uAA77-\uAA79\uAB5B\uAB6A\uAB6B\uFB29\uFBB2-\uFBC2\uFD40-\uFD4F\uFDCF\uFDFC-\uFDFF\uFE62\uFE64-\uFE66\uFE69\uFF04\uFF0B\uFF1C-\uFF1E\uFF3E\uFF40\uFF5C\uFF5E\uFFE0-\uFFE6\uFFE8-\uFFEE\uFFFC\uFFFD]|\uD800[\uDD37-\uDD3F\uDD79-\uDD89\uDD8C-\uDD8E\uDD90-\uDD9C\uDDA0\uDDD0-\uDDFC]|\uD802[\uDC77\uDC78\uDEC8]|\uD805\uDF3F|\uD807[\uDFD5-\uDFF1]|\uD81A[\uDF3C-\uDF3F\uDF45]|\uD82F\uDC9C|\uD833[\uDF50-\uDFC3]|\uD834[\uDC00-\uDCF5\uDD00-\uDD26\uDD29-\uDD64\uDD6A-\uDD6C\uDD83\uDD84\uDD8C-\uDDA9\uDDAE-\uDDEA\uDE00-\uDE41\uDE45\uDF00-\uDF56]|\uD835[\uDEC1\uDEDB\uDEFB\uDF15\uDF35\uDF4F\uDF6F\uDF89\uDFA9\uDFC3]|\uD836[\uDC00-\uDDFF\uDE37-\uDE3A\uDE6D-\uDE74\uDE76-\uDE83\uDE85\uDE86]|\uD838[\uDD4F\uDEFF]|\uD83B[\uDCAC\uDCB0\uDD2E\uDEF0\uDEF1]|\uD83C[\uDC00-\uDC2B\uDC30-\uDC93\uDCA0-\uDCAE\uDCB1-\uDCBF\uDCC1-\uDCCF\uDCD1-\uDCF5\uDD0D-\uDDAD\uDDE6-\uDE02\uDE10-\uDE3B\uDE40-\uDE48\uDE50\uDE51\uDE60-\uDE65\uDF00-\uDFFF]|\uD83D[\uDC00-\uDED7\uDEDC-\uDEEC\uDEF0-\uDEFC\uDF00-\uDF76\uDF7B-\uDFD9\uDFE0-\uDFEB\uDFF0]|\uD83E[\uDC00-\uDC0B\uDC10-\uDC47\uDC50-\uDC59\uDC60-\uDC87\uDC90-\uDCAD\uDCB0\uDCB1\uDD00-\uDE53\uDE60-\uDE6D\uDE70-\uDE7C\uDE80-\uDE88\uDE90-\uDEBD\uDEBF-\uDEC5\uDECE-\uDEDB\uDEE0-\uDEE8\uDEF0-\uDEF8\uDF00-\uDF92\uDF94-\uDFCA]/, ft = /[ \xA0\u1680\u2000-\u200A\u2028\u2029\u202F\u205F\u3000]/, pt = /* @__PURE__ */ t({
	Any: () => st,
	Cc: () => ct,
	Cf: () => lt,
	P: () => ut,
	S: () => dt,
	Z: () => ft
}), mt = new Uint16Array("ᵁ<Õıʊҝջאٵ۞ޢߖࠏ੊ઑඡ๭༉༦჊ረዡᐕᒝᓃᓟᔥ\0\0\0\0\0\0ᕫᛍᦍᰒᷝ὾⁠↰⊍⏀⏻⑂⠤⤒ⴈ⹈⿎〖㊺㘹㞬㣾㨨㩱㫠㬮ࠀEMabcfglmnoprstu\\bfms¦³¹ÈÏlig耻Æ䃆P耻&䀦cute耻Á䃁reve;䄂Āiyx}rc耻Â䃂;䐐r;쀀𝔄rave耻À䃀pha;䎑acr;䄀d;橓Āgp¡on;䄄f;쀀𝔸plyFunction;恡ing耻Å䃅Ācs¾Ãr;쀀𝒜ign;扔ilde耻Ã䃃ml耻Ä䃄ЀaceforsuåûþėĜĢħĪĀcrêòkslash;或Ŷöø;櫧ed;挆y;䐑ƀcrtąċĔause;戵noullis;愬a;䎒r;쀀𝔅pf;쀀𝔹eve;䋘còēmpeq;扎܀HOacdefhilorsuōőŖƀƞƢƵƷƺǜȕɳɸɾcy;䐧PY耻©䂩ƀcpyŝŢźute;䄆Ā;iŧŨ拒talDifferentialD;慅leys;愭ȀaeioƉƎƔƘron;䄌dil耻Ç䃇rc;䄈nint;戰ot;䄊ĀdnƧƭilla;䂸terDot;䂷òſi;䎧rcleȀDMPTǇǋǑǖot;抙inus;抖lus;投imes;抗oĀcsǢǸkwiseContourIntegral;戲eCurlyĀDQȃȏoubleQuote;思uote;怙ȀlnpuȞȨɇɕonĀ;eȥȦ户;橴ƀgitȯȶȺruent;扡nt;戯ourIntegral;戮ĀfrɌɎ;愂oduct;成nterClockwiseContourIntegral;戳oss;樯cr;쀀𝒞pĀ;Cʄʅ拓ap;才րDJSZacefiosʠʬʰʴʸˋ˗ˡ˦̳ҍĀ;oŹʥtrahd;椑cy;䐂cy;䐅cy;䐏ƀgrsʿ˄ˇger;怡r;憡hv;櫤Āayː˕ron;䄎;䐔lĀ;t˝˞戇a;䎔r;쀀𝔇Āaf˫̧Ācm˰̢riticalȀADGT̖̜̀̆cute;䂴oŴ̋̍;䋙bleAcute;䋝rave;䁠ilde;䋜ond;拄ferentialD;慆Ѱ̽\0\0\0͔͂\0Ѕf;쀀𝔻ƀ;DE͈͉͍䂨ot;惜qual;扐blèCDLRUVͣͲ΂ϏϢϸontourIntegraìȹoɴ͹\0\0ͻ»͉nArrow;懓Āeo·ΤftƀARTΐΖΡrrow;懐ightArrow;懔eåˊngĀLRΫτeftĀARγιrrow;柸ightArrow;柺ightArrow;柹ightĀATϘϞrrow;懒ee;抨pɁϩ\0\0ϯrrow;懑ownArrow;懕erticalBar;戥ǹABLRTaВЪаўѿͼrrowƀ;BUНОТ憓ar;椓pArrow;懵reve;䌑eft˒к\0ц\0ѐightVector;楐eeVector;楞ectorĀ;Bљњ憽ar;楖ightǔѧ\0ѱeeVector;楟ectorĀ;BѺѻ懁ar;楗eeĀ;A҆҇护rrow;憧ĀctҒҗr;쀀𝒟rok;䄐ࠀNTacdfglmopqstuxҽӀӄӋӞӢӧӮӵԡԯԶՒ՝ՠեG;䅊H耻Ð䃐cute耻É䃉ƀaiyӒӗӜron;䄚rc耻Ê䃊;䐭ot;䄖r;쀀𝔈rave耻È䃈ement;戈ĀapӺӾcr;䄒tyɓԆ\0\0ԒmallSquare;旻erySmallSquare;斫ĀgpԦԪon;䄘f;쀀𝔼silon;䎕uĀaiԼՉlĀ;TՂՃ橵ilde;扂librium;懌Āci՗՚r;愰m;橳a;䎗ml耻Ë䃋Āipժկsts;戃onentialE;慇ʀcfiosօֈ֍ֲ׌y;䐤r;쀀𝔉lledɓ֗\0\0֣mallSquare;旼erySmallSquare;斪Ͱֺ\0ֿ\0\0ׄf;쀀𝔽All;戀riertrf;愱cò׋؀JTabcdfgorstר׬ׯ׺؀ؒؖ؛؝أ٬ٲcy;䐃耻>䀾mmaĀ;d׷׸䎓;䏜reve;䄞ƀeiy؇،ؐdil;䄢rc;䄜;䐓ot;䄠r;쀀𝔊;拙pf;쀀𝔾eater̀EFGLSTصلَٖٛ٦qualĀ;Lؾؿ扥ess;招ullEqual;执reater;檢ess;扷lantEqual;橾ilde;扳cr;쀀𝒢;扫ЀAacfiosuڅڋږڛڞڪھۊRDcy;䐪Āctڐڔek;䋇;䁞irc;䄤r;愌lbertSpace;愋ǰگ\0ڲf;愍izontalLine;攀Āctۃۅòکrok;䄦mpńېۘownHumðįqual;扏܀EJOacdfgmnostuۺ۾܃܇܎ܚܞܡܨ݄ݸދޏޕcy;䐕lig;䄲cy;䐁cute耻Í䃍Āiyܓܘrc耻Î䃎;䐘ot;䄰r;愑rave耻Ì䃌ƀ;apܠܯܿĀcgܴܷr;䄪inaryI;慈lieóϝǴ݉\0ݢĀ;eݍݎ戬Āgrݓݘral;戫section;拂isibleĀCTݬݲomma;恣imes;恢ƀgptݿރވon;䄮f;쀀𝕀a;䎙cr;愐ilde;䄨ǫޚ\0ޞcy;䐆l耻Ï䃏ʀcfosuެ޷޼߂ߐĀiyޱ޵rc;䄴;䐙r;쀀𝔍pf;쀀𝕁ǣ߇\0ߌr;쀀𝒥rcy;䐈kcy;䐄΀HJacfosߤߨ߽߬߱ࠂࠈcy;䐥cy;䐌ppa;䎚Āey߶߻dil;䄶;䐚r;쀀𝔎pf;쀀𝕂cr;쀀𝒦րJTaceflmostࠥࠩࠬࡐࡣ঳সে্਷ੇcy;䐉耻<䀼ʀcmnpr࠷࠼ࡁࡄࡍute;䄹bda;䎛g;柪lacetrf;愒r;憞ƀaeyࡗ࡜ࡡron;䄽dil;䄻;䐛Āfsࡨ॰tԀACDFRTUVarࡾࢩࢱࣦ࣠ࣼयज़ΐ४Ānrࢃ࢏gleBracket;柨rowƀ;BR࢙࢚࢞憐ar;懤ightArrow;懆eiling;挈oǵࢷ\0ࣃbleBracket;柦nǔࣈ\0࣒eeVector;楡ectorĀ;Bࣛࣜ懃ar;楙loor;挊ightĀAV࣯ࣵrrow;憔ector;楎Āerँगeƀ;AVउऊऐ抣rrow;憤ector;楚iangleƀ;BEतथऩ抲ar;槏qual;抴pƀDTVषूौownVector;楑eeVector;楠ectorĀ;Bॖॗ憿ar;楘ectorĀ;B॥०憼ar;楒ightáΜs̀EFGLSTॾঋকঝঢভqualGreater;拚ullEqual;扦reater;扶ess;檡lantEqual;橽ilde;扲r;쀀𝔏Ā;eঽা拘ftarrow;懚idot;䄿ƀnpw৔ਖਛgȀLRlr৞৷ਂਐeftĀAR০৬rrow;柵ightArrow;柷ightArrow;柶eftĀarγਊightáοightáϊf;쀀𝕃erĀLRਢਬeftArrow;憙ightArrow;憘ƀchtਾੀੂòࡌ;憰rok;䅁;扪Ѐacefiosuਗ਼੝੠੷੼અઋ઎p;椅y;䐜Ādl੥੯iumSpace;恟lintrf;愳r;쀀𝔐nusPlus;戓pf;쀀𝕄cò੶;䎜ҀJacefostuણધભીଔଙඑ඗ඞcy;䐊cute;䅃ƀaey઴હાron;䅇dil;䅅;䐝ƀgswે૰଎ativeƀMTV૓૟૨ediumSpace;怋hiĀcn૦૘ë૙eryThiî૙tedĀGL૸ଆreaterGreateòٳessLesóੈLine;䀊r;쀀𝔑ȀBnptଢନଷ଺reak;恠BreakingSpace;䂠f;愕ڀ;CDEGHLNPRSTV୕ୖ୪୼஡௫ఄ౞಄ದ೘ൡඅ櫬Āou୛୤ngruent;扢pCap;扭oubleVerticalBar;戦ƀlqxஃஊ஛ement;戉ualĀ;Tஒஓ扠ilde;쀀≂̸ists;戄reater΀;EFGLSTஶஷ஽௉௓௘௥扯qual;扱ullEqual;쀀≧̸reater;쀀≫̸ess;批lantEqual;쀀⩾̸ilde;扵umpń௲௽ownHump;쀀≎̸qual;쀀≏̸eĀfsఊధtTriangleƀ;BEచఛడ拪ar;쀀⧏̸qual;括s̀;EGLSTవశ఼ౄోౘ扮qual;扰reater;扸ess;쀀≪̸lantEqual;쀀⩽̸ilde;扴estedĀGL౨౹reaterGreater;쀀⪢̸essLess;쀀⪡̸recedesƀ;ESಒಓಛ技qual;쀀⪯̸lantEqual;拠ĀeiಫಹverseElement;戌ghtTriangleƀ;BEೋೌ೒拫ar;쀀⧐̸qual;拭ĀquೝഌuareSuĀbp೨೹setĀ;E೰ೳ쀀⊏̸qual;拢ersetĀ;Eഃആ쀀⊐̸qual;拣ƀbcpഓതൎsetĀ;Eഛഞ쀀⊂⃒qual;抈ceedsȀ;ESTലള഻െ抁qual;쀀⪰̸lantEqual;拡ilde;쀀≿̸ersetĀ;E൘൛쀀⊃⃒qual;抉ildeȀ;EFT൮൯൵ൿ扁qual;扄ullEqual;扇ilde;扉erticalBar;戤cr;쀀𝒩ilde耻Ñ䃑;䎝܀Eacdfgmoprstuvලෂ෉෕ෛ෠෧෼ขภยา฿ไlig;䅒cute耻Ó䃓Āiy෎ීrc耻Ô䃔;䐞blac;䅐r;쀀𝔒rave耻Ò䃒ƀaei෮ෲ෶cr;䅌ga;䎩cron;䎟pf;쀀𝕆enCurlyĀDQฎบoubleQuote;怜uote;怘;橔Āclวฬr;쀀𝒪ash耻Ø䃘iŬื฼de耻Õ䃕es;樷ml耻Ö䃖erĀBP๋๠Āar๐๓r;怾acĀek๚๜;揞et;掴arenthesis;揜Ҁacfhilors๿ງຊຏຒດຝະ໼rtialD;戂y;䐟r;쀀𝔓i;䎦;䎠usMinus;䂱Āipຢອncareplanåڝf;愙Ȁ;eio຺ູ໠໤檻cedesȀ;EST່້໏໚扺qual;檯lantEqual;扼ilde;找me;怳Ādp໩໮uct;戏ortionĀ;aȥ໹l;戝Āci༁༆r;쀀𝒫;䎨ȀUfos༑༖༛༟OT耻\"䀢r;쀀𝔔pf;愚cr;쀀𝒬؀BEacefhiorsu༾གྷཇའཱིྦྷྪྭ႖ႩႴႾarr;椐G耻®䂮ƀcnrཎནབute;䅔g;柫rĀ;tཛྷཝ憠l;椖ƀaeyཧཬཱron;䅘dil;䅖;䐠Ā;vླྀཹ愜erseĀEUྂྙĀlq྇ྎement;戋uilibrium;懋pEquilibrium;楯r»ཹo;䎡ghtЀACDFTUVa࿁࿫࿳ဢဨၛႇϘĀnr࿆࿒gleBracket;柩rowƀ;BL࿜࿝࿡憒ar;懥eftArrow;懄eiling;按oǵ࿹\0စbleBracket;柧nǔည\0နeeVector;楝ectorĀ;Bဝသ懂ar;楕loor;挋Āerိ၃eƀ;AVဵံြ抢rrow;憦ector;楛iangleƀ;BEၐၑၕ抳ar;槐qual;抵pƀDTVၣၮၸownVector;楏eeVector;楜ectorĀ;Bႂႃ憾ar;楔ectorĀ;B႑႒懀ar;楓Āpuႛ႞f;愝ndImplies;楰ightarrow;懛ĀchႹႼr;愛;憱leDelayed;槴ڀHOacfhimoqstuფჱჷჽᄙᄞᅑᅖᅡᅧᆵᆻᆿĀCcჩხHcy;䐩y;䐨FTcy;䐬cute;䅚ʀ;aeiyᄈᄉᄎᄓᄗ檼ron;䅠dil;䅞rc;䅜;䐡r;쀀𝔖ortȀDLRUᄪᄴᄾᅉownArrow»ОeftArrow»࢚ightArrow»࿝pArrow;憑gma;䎣allCircle;战pf;쀀𝕊ɲᅭ\0\0ᅰt;戚areȀ;ISUᅻᅼᆉᆯ斡ntersection;抓uĀbpᆏᆞsetĀ;Eᆗᆘ抏qual;抑ersetĀ;Eᆨᆩ抐qual;抒nion;抔cr;쀀𝒮ar;拆ȀbcmpᇈᇛሉላĀ;sᇍᇎ拐etĀ;Eᇍᇕqual;抆ĀchᇠህeedsȀ;ESTᇭᇮᇴᇿ扻qual;檰lantEqual;扽ilde;承Tháྌ;我ƀ;esሒሓሣ拑rsetĀ;Eሜም抃qual;抇et»ሓրHRSacfhiorsሾቄ቉ቕ቞ቱቶኟዂወዑORN耻Þ䃞ADE;愢ĀHc቎ቒcy;䐋y;䐦Ābuቚቜ;䀉;䎤ƀaeyብቪቯron;䅤dil;䅢;䐢r;쀀𝔗Āeiቻ኉ǲኀ\0ኇefore;戴a;䎘Ācn኎ኘkSpace;쀀  Space;怉ldeȀ;EFTካኬኲኼ戼qual;扃ullEqual;扅ilde;扈pf;쀀𝕋ipleDot;惛Āctዖዛr;쀀𝒯rok;䅦ૡዷጎጚጦ\0ጬጱ\0\0\0\0\0ጸጽ፷ᎅ\0᏿ᐄᐊᐐĀcrዻጁute耻Ú䃚rĀ;oጇገ憟cir;楉rǣጓ\0጖y;䐎ve;䅬Āiyጞጣrc耻Û䃛;䐣blac;䅰r;쀀𝔘rave耻Ù䃙acr;䅪Ādiፁ፩erĀBPፈ፝Āarፍፐr;䁟acĀekፗፙ;揟et;掵arenthesis;揝onĀ;P፰፱拃lus;抎Āgp፻፿on;䅲f;쀀𝕌ЀADETadps᎕ᎮᎸᏄϨᏒᏗᏳrrowƀ;BDᅐᎠᎤar;椒ownArrow;懅ownArrow;憕quilibrium;楮eeĀ;AᏋᏌ报rrow;憥ownáϳerĀLRᏞᏨeftArrow;憖ightArrow;憗iĀ;lᏹᏺ䏒on;䎥ing;䅮cr;쀀𝒰ilde;䅨ml耻Ü䃜ҀDbcdefosvᐧᐬᐰᐳᐾᒅᒊᒐᒖash;披ar;櫫y;䐒ashĀ;lᐻᐼ抩;櫦Āerᑃᑅ;拁ƀbtyᑌᑐᑺar;怖Ā;iᑏᑕcalȀBLSTᑡᑥᑪᑴar;戣ine;䁼eparator;杘ilde;所ThinSpace;怊r;쀀𝔙pf;쀀𝕍cr;쀀𝒱dash;抪ʀcefosᒧᒬᒱᒶᒼirc;䅴dge;拀r;쀀𝔚pf;쀀𝕎cr;쀀𝒲Ȁfiosᓋᓐᓒᓘr;쀀𝔛;䎞pf;쀀𝕏cr;쀀𝒳ҀAIUacfosuᓱᓵᓹᓽᔄᔏᔔᔚᔠcy;䐯cy;䐇cy;䐮cute耻Ý䃝Āiyᔉᔍrc;䅶;䐫r;쀀𝔜pf;쀀𝕐cr;쀀𝒴ml;䅸ЀHacdefosᔵᔹᔿᕋᕏᕝᕠᕤcy;䐖cute;䅹Āayᕄᕉron;䅽;䐗ot;䅻ǲᕔ\0ᕛoWidtè૙a;䎖r;愨pf;愤cr;쀀𝒵௡ᖃᖊᖐ\0ᖰᖶᖿ\0\0\0\0ᗆᗛᗫᙟ᙭\0ᚕ᚛ᚲᚹ\0ᚾcute耻á䃡reve;䄃̀;Ediuyᖜᖝᖡᖣᖨᖭ戾;쀀∾̳;房rc耻â䃢te肻´̆;䐰lig耻æ䃦Ā;r²ᖺ;쀀𝔞rave耻à䃠ĀepᗊᗖĀfpᗏᗔsym;愵èᗓha;䎱ĀapᗟcĀclᗤᗧr;䄁g;樿ɤᗰ\0\0ᘊʀ;adsvᗺᗻᗿᘁᘇ戧nd;橕;橜lope;橘;橚΀;elmrszᘘᘙᘛᘞᘿᙏᙙ戠;榤e»ᘙsdĀ;aᘥᘦ戡ѡᘰᘲᘴᘶᘸᘺᘼᘾ;榨;榩;榪;榫;榬;榭;榮;榯tĀ;vᙅᙆ戟bĀ;dᙌᙍ抾;榝Āptᙔᙗh;戢»¹arr;捼Āgpᙣᙧon;䄅f;쀀𝕒΀;Eaeiop዁ᙻᙽᚂᚄᚇᚊ;橰cir;橯;扊d;手s;䀧roxĀ;e዁ᚒñᚃing耻å䃥ƀctyᚡᚦᚨr;쀀𝒶;䀪mpĀ;e዁ᚯñʈilde耻ã䃣ml耻ä䃤Āciᛂᛈoninôɲnt;樑ࠀNabcdefiklnoprsu᛭ᛱᜰ᜼ᝃᝈ᝸᝽០៦ᠹᡐᜍ᤽᥈ᥰot;櫭Ācrᛶ᜞kȀcepsᜀᜅᜍᜓong;扌psilon;䏶rime;怵imĀ;e᜚᜛戽q;拍Ŷᜢᜦee;抽edĀ;gᜬᜭ挅e»ᜭrkĀ;t፜᜷brk;掶Āoyᜁᝁ;䐱quo;怞ʀcmprtᝓ᝛ᝡᝤᝨausĀ;eĊĉptyv;榰séᜌnoõēƀahwᝯ᝱ᝳ;䎲;愶een;扬r;쀀𝔟g΀costuvwឍឝឳេ៕៛៞ƀaiuបពរðݠrc;旯p»፱ƀdptឤឨឭot;樀lus;樁imes;樂ɱឹ\0\0ើcup;樆ar;昅riangleĀdu៍្own;施p;斳plus;樄eåᑄåᒭarow;植ƀako៭ᠦᠵĀcn៲ᠣkƀlst៺֫᠂ozenge;槫riangleȀ;dlr᠒᠓᠘᠝斴own;斾eft;旂ight;斸k;搣Ʊᠫ\0ᠳƲᠯ\0ᠱ;斒;斑4;斓ck;斈ĀeoᠾᡍĀ;qᡃᡆ쀀=⃥uiv;쀀≡⃥t;挐Ȁptwxᡙᡞᡧᡬf;쀀𝕓Ā;tᏋᡣom»Ꮜtie;拈؀DHUVbdhmptuvᢅᢖᢪᢻᣗᣛᣬ᣿ᤅᤊᤐᤡȀLRlrᢎᢐᢒᢔ;敗;敔;敖;敓ʀ;DUduᢡᢢᢤᢦᢨ敐;敦;敩;敤;敧ȀLRlrᢳᢵᢷᢹ;敝;敚;敜;教΀;HLRhlrᣊᣋᣍᣏᣑᣓᣕ救;敬;散;敠;敫;敢;敟ox;槉ȀLRlrᣤᣦᣨᣪ;敕;敒;攐;攌ʀ;DUduڽ᣷᣹᣻᣽;敥;敨;攬;攴inus;抟lus;択imes;抠ȀLRlrᤙᤛᤝ᤟;敛;敘;攘;攔΀;HLRhlrᤰᤱᤳᤵᤷ᤻᤹攂;敪;敡;敞;攼;攤;攜Āevģ᥂bar耻¦䂦Ȁceioᥑᥖᥚᥠr;쀀𝒷mi;恏mĀ;e᜚᜜lƀ;bhᥨᥩᥫ䁜;槅sub;柈Ŭᥴ᥾lĀ;e᥹᥺怢t»᥺pƀ;Eeįᦅᦇ;檮Ā;qۜۛೡᦧ\0᧨ᨑᨕᨲ\0ᨷᩐ\0\0᪴\0\0᫁\0\0ᬡᬮ᭍᭒\0᯽\0ᰌƀcpr᦭ᦲ᧝ute;䄇̀;abcdsᦿᧀᧄ᧊᧕᧙戩nd;橄rcup;橉Āau᧏᧒p;橋p;橇ot;橀;쀀∩︀Āeo᧢᧥t;恁îړȀaeiu᧰᧻ᨁᨅǰ᧵\0᧸s;橍on;䄍dil耻ç䃧rc;䄉psĀ;sᨌᨍ橌m;橐ot;䄋ƀdmnᨛᨠᨦil肻¸ƭptyv;榲t脀¢;eᨭᨮ䂢räƲr;쀀𝔠ƀceiᨽᩀᩍy;䑇ckĀ;mᩇᩈ朓ark»ᩈ;䏇r΀;Ecefms᩟᩠ᩢᩫ᪤᪪᪮旋;槃ƀ;elᩩᩪᩭ䋆q;扗eɡᩴ\0\0᪈rrowĀlr᩼᪁eft;憺ight;憻ʀRSacd᪒᪔᪖᪚᪟»ཇ;擈st;抛irc;抚ash;抝nint;樐id;櫯cir;槂ubsĀ;u᪻᪼晣it»᪼ˬ᫇᫔᫺\0ᬊonĀ;eᫍᫎ䀺Ā;qÇÆɭ᫙\0\0᫢aĀ;t᫞᫟䀬;䁀ƀ;fl᫨᫩᫫戁îᅠeĀmx᫱᫶ent»᫩eóɍǧ᫾\0ᬇĀ;dኻᬂot;橭nôɆƀfryᬐᬔᬗ;쀀𝕔oäɔ脀©;sŕᬝr;愗Āaoᬥᬩrr;憵ss;朗Ācuᬲᬷr;쀀𝒸Ābpᬼ᭄Ā;eᭁᭂ櫏;櫑Ā;eᭉᭊ櫐;櫒dot;拯΀delprvw᭠᭬᭷ᮂᮬᯔ᯹arrĀlr᭨᭪;椸;椵ɰ᭲\0\0᭵r;拞c;拟arrĀ;p᭿ᮀ憶;椽̀;bcdosᮏᮐᮖᮡᮥᮨ截rcap;橈Āauᮛᮞp;橆p;橊ot;抍r;橅;쀀∪︀Ȁalrv᮵ᮿᯞᯣrrĀ;mᮼᮽ憷;椼yƀevwᯇᯔᯘqɰᯎ\0\0ᯒreã᭳uã᭵ee;拎edge;拏en耻¤䂤earrowĀlrᯮ᯳eft»ᮀight»ᮽeäᯝĀciᰁᰇoninôǷnt;戱lcty;挭ঀAHabcdefhijlorstuwz᰸᰻᰿ᱝᱩᱵᲊᲞᲬᲷ᳻᳿ᴍᵻᶑᶫᶻ᷆᷍rò΁ar;楥Ȁglrs᱈ᱍ᱒᱔ger;怠eth;愸òᄳhĀ;vᱚᱛ怐»ऊūᱡᱧarow;椏aã̕Āayᱮᱳron;䄏;䐴ƀ;ao̲ᱼᲄĀgrʿᲁr;懊tseq;橷ƀglmᲑᲔᲘ耻°䂰ta;䎴ptyv;榱ĀirᲣᲨsht;楿;쀀𝔡arĀlrᲳᲵ»ࣜ»သʀaegsv᳂͸᳖᳜᳠mƀ;oș᳊᳔ndĀ;ș᳑uit;晦amma;䏝in;拲ƀ;io᳧᳨᳸䃷de脀÷;o᳧ᳰntimes;拇nø᳷cy;䑒cɯᴆ\0\0ᴊrn;挞op;挍ʀlptuwᴘᴝᴢᵉᵕlar;䀤f;쀀𝕕ʀ;emps̋ᴭᴷᴽᵂqĀ;d͒ᴳot;扑inus;戸lus;戔quare;抡blebarwedgåúnƀadhᄮᵝᵧownarrowóᲃarpoonĀlrᵲᵶefôᲴighôᲶŢᵿᶅkaro÷གɯᶊ\0\0ᶎrn;挟op;挌ƀcotᶘᶣᶦĀryᶝᶡ;쀀𝒹;䑕l;槶rok;䄑Ādrᶰᶴot;拱iĀ;fᶺ᠖斿Āah᷀᷃ròЩaòྦangle;榦Āci᷒ᷕy;䑟grarr;柿ऀDacdefglmnopqrstuxḁḉḙḸոḼṉṡṾấắẽỡἪἷὄ὎὚ĀDoḆᴴoôᲉĀcsḎḔute耻é䃩ter;橮ȀaioyḢḧḱḶron;䄛rĀ;cḭḮ扖耻ê䃪lon;払;䑍ot;䄗ĀDrṁṅot;扒;쀀𝔢ƀ;rsṐṑṗ檚ave耻è䃨Ā;dṜṝ檖ot;檘Ȁ;ilsṪṫṲṴ檙nters;揧;愓Ā;dṹṺ檕ot;檗ƀapsẅẉẗcr;䄓tyƀ;svẒẓẕ戅et»ẓpĀ1;ẝẤĳạả;怄;怅怃ĀgsẪẬ;䅋p;怂ĀgpẴẸon;䄙f;쀀𝕖ƀalsỄỎỒrĀ;sỊị拕l;槣us;橱iƀ;lvỚớở䎵on»ớ;䏵ȀcsuvỪỳἋἣĀioữḱrc»Ḯɩỹ\0\0ỻíՈantĀglἂἆtr»ṝess»Ṻƀaeiἒ἖Ἒls;䀽st;扟vĀ;DȵἠD;橸parsl;槥ĀDaἯἳot;打rr;楱ƀcdiἾὁỸr;愯oô͒ĀahὉὋ;䎷耻ð䃰Āmrὓὗl耻ë䃫o;悬ƀcipὡὤὧl;䀡sôծĀeoὬὴctatioîՙnentialåչৡᾒ\0ᾞ\0ᾡᾧ\0\0ῆῌ\0ΐ\0ῦῪ \0 ⁚llingdotseñṄy;䑄male;晀ƀilrᾭᾳ῁lig;耀ﬃɩᾹ\0\0᾽g;耀ﬀig;耀ﬄ;쀀𝔣lig;耀ﬁlig;쀀fjƀaltῙ῜ῡt;晭ig;耀ﬂns;斱of;䆒ǰ΅\0ῳf;쀀𝕗ĀakֿῷĀ;vῼ´拔;櫙artint;樍Āao‌⁕Ācs‑⁒α‚‰‸⁅⁈\0⁐β•‥‧‪‬\0‮耻½䂽;慓耻¼䂼;慕;慙;慛Ƴ‴\0‶;慔;慖ʴ‾⁁\0\0⁃耻¾䂾;慗;慜5;慘ƶ⁌\0⁎;慚;慝8;慞l;恄wn;挢cr;쀀𝒻ࢀEabcdefgijlnorstv₂₉₟₥₰₴⃰⃵⃺⃿℃ℒℸ̗ℾ⅒↞Ā;lٍ₇;檌ƀcmpₐₕ₝ute;䇵maĀ;dₜ᳚䎳;檆reve;䄟Āiy₪₮rc;䄝;䐳ot;䄡Ȁ;lqsؾق₽⃉ƀ;qsؾٌ⃄lanô٥Ȁ;cdl٥⃒⃥⃕c;檩otĀ;o⃜⃝檀Ā;l⃢⃣檂;檄Ā;e⃪⃭쀀⋛︀s;檔r;쀀𝔤Ā;gٳ؛mel;愷cy;䑓Ȁ;Eajٚℌℎℐ;檒;檥;檤ȀEaesℛℝ℩ℴ;扩pĀ;p℣ℤ檊rox»ℤĀ;q℮ℯ檈Ā;q℮ℛim;拧pf;쀀𝕘Āci⅃ⅆr;愊mƀ;el٫ⅎ⅐;檎;檐茀>;cdlqr׮ⅠⅪⅮⅳⅹĀciⅥⅧ;檧r;橺ot;拗Par;榕uest;橼ʀadelsↄⅪ←ٖ↛ǰ↉\0↎proø₞r;楸qĀlqؿ↖lesó₈ií٫Āen↣↭rtneqq;쀀≩︀Å↪ԀAabcefkosy⇄⇇⇱⇵⇺∘∝∯≨≽ròΠȀilmr⇐⇔⇗⇛rsðᒄf»․ilôکĀdr⇠⇤cy;䑊ƀ;cwࣴ⇫⇯ir;楈;憭ar;意irc;䄥ƀalr∁∎∓rtsĀ;u∉∊晥it»∊lip;怦con;抹r;쀀𝔥sĀew∣∩arow;椥arow;椦ʀamopr∺∾≃≞≣rr;懿tht;戻kĀlr≉≓eftarrow;憩ightarrow;憪f;쀀𝕙bar;怕ƀclt≯≴≸r;쀀𝒽asè⇴rok;䄧Ābp⊂⊇ull;恃hen»ᱛૡ⊣\0⊪\0⊸⋅⋎\0⋕⋳\0\0⋸⌢⍧⍢⍿\0⎆⎪⎴cute耻í䃭ƀ;iyݱ⊰⊵rc耻î䃮;䐸Ācx⊼⊿y;䐵cl耻¡䂡ĀfrΟ⋉;쀀𝔦rave耻ì䃬Ȁ;inoܾ⋝⋩⋮Āin⋢⋦nt;樌t;戭fin;槜ta;愩lig;䄳ƀaop⋾⌚⌝ƀcgt⌅⌈⌗r;䄫ƀelpܟ⌏⌓inåގarôܠh;䄱f;抷ed;䆵ʀ;cfotӴ⌬⌱⌽⍁are;愅inĀ;t⌸⌹戞ie;槝doô⌙ʀ;celpݗ⍌⍐⍛⍡al;抺Āgr⍕⍙eróᕣã⍍arhk;樗rod;樼Ȁcgpt⍯⍲⍶⍻y;䑑on;䄯f;쀀𝕚a;䎹uest耻¿䂿Āci⎊⎏r;쀀𝒾nʀ;EdsvӴ⎛⎝⎡ӳ;拹ot;拵Ā;v⎦⎧拴;拳Ā;iݷ⎮lde;䄩ǫ⎸\0⎼cy;䑖l耻ï䃯̀cfmosu⏌⏗⏜⏡⏧⏵Āiy⏑⏕rc;䄵;䐹r;쀀𝔧ath;䈷pf;쀀𝕛ǣ⏬\0⏱r;쀀𝒿rcy;䑘kcy;䑔Ѐacfghjos␋␖␢␧␭␱␵␻ppaĀ;v␓␔䎺;䏰Āey␛␠dil;䄷;䐺r;쀀𝔨reen;䄸cy;䑅cy;䑜pf;쀀𝕜cr;쀀𝓀஀ABEHabcdefghjlmnoprstuv⑰⒁⒆⒍⒑┎┽╚▀♎♞♥♹♽⚚⚲⛘❝❨➋⟀⠁⠒ƀart⑷⑺⑼rò৆òΕail;椛arr;椎Ā;gঔ⒋;檋ar;楢ॣ⒥\0⒪\0⒱\0\0\0\0\0⒵Ⓔ\0ⓆⓈⓍ\0⓹ute;䄺mptyv;榴raîࡌbda;䎻gƀ;dlࢎⓁⓃ;榑åࢎ;檅uo耻«䂫rЀ;bfhlpst࢙ⓞⓦⓩ⓫⓮⓱⓵Ā;f࢝ⓣs;椟s;椝ë≒p;憫l;椹im;楳l;憢ƀ;ae⓿─┄檫il;椙Ā;s┉┊檭;쀀⪭︀ƀabr┕┙┝rr;椌rk;杲Āak┢┬cĀek┨┪;䁻;䁛Āes┱┳;榋lĀdu┹┻;榏;榍Ȁaeuy╆╋╖╘ron;䄾Ādi═╔il;䄼ìࢰâ┩;䐻Ȁcqrs╣╦╭╽a;椶uoĀ;rนᝆĀdu╲╷har;楧shar;楋h;憲ʀ;fgqs▋▌উ◳◿扤tʀahlrt▘▤▷◂◨rrowĀ;t࢙□aé⓶arpoonĀdu▯▴own»њp»०eftarrows;懇ightƀahs◍◖◞rrowĀ;sࣴࢧarpoonó྘quigarro÷⇰hreetimes;拋ƀ;qs▋ও◺lanôবʀ;cdgsব☊☍☝☨c;檨otĀ;o☔☕橿Ā;r☚☛檁;檃Ā;e☢☥쀀⋚︀s;檓ʀadegs☳☹☽♉♋pproøⓆot;拖qĀgq♃♅ôউgtò⒌ôছiíলƀilr♕࣡♚sht;楼;쀀𝔩Ā;Eজ♣;檑š♩♶rĀdu▲♮Ā;l॥♳;楪lk;斄cy;䑙ʀ;achtੈ⚈⚋⚑⚖rò◁orneòᴈard;楫ri;旺Āio⚟⚤dot;䅀ustĀ;a⚬⚭掰che»⚭ȀEaes⚻⚽⛉⛔;扨pĀ;p⛃⛄檉rox»⛄Ā;q⛎⛏檇Ā;q⛎⚻im;拦Ѐabnoptwz⛩⛴⛷✚✯❁❇❐Ānr⛮⛱g;柬r;懽rëࣁgƀlmr⛿✍✔eftĀar০✇ightá৲apsto;柼ightá৽parrowĀlr✥✩efô⓭ight;憬ƀafl✶✹✽r;榅;쀀𝕝us;樭imes;樴š❋❏st;戗áፎƀ;ef❗❘᠀旊nge»❘arĀ;l❤❥䀨t;榓ʀachmt❳❶❼➅➇ròࢨorneòᶌarĀ;d྘➃;業;怎ri;抿̀achiqt➘➝ੀ➢➮➻quo;怹r;쀀𝓁mƀ;egল➪➬;檍;檏Ābu┪➳oĀ;rฟ➹;怚rok;䅂萀<;cdhilqrࠫ⟒☹⟜⟠⟥⟪⟰Āci⟗⟙;檦r;橹reå◲mes;拉arr;楶uest;橻ĀPi⟵⟹ar;榖ƀ;ef⠀भ᠛旃rĀdu⠇⠍shar;楊har;楦Āen⠗⠡rtneqq;쀀≨︀Å⠞܀Dacdefhilnopsu⡀⡅⢂⢎⢓⢠⢥⢨⣚⣢⣤ઃ⣳⤂Dot;戺Ȁclpr⡎⡒⡣⡽r耻¯䂯Āet⡗⡙;時Ā;e⡞⡟朠se»⡟Ā;sျ⡨toȀ;dluျ⡳⡷⡻owîҌefôएðᏑker;斮Āoy⢇⢌mma;権;䐼ash;怔asuredangle»ᘦr;쀀𝔪o;愧ƀcdn⢯⢴⣉ro耻µ䂵Ȁ;acdᑤ⢽⣀⣄sôᚧir;櫰ot肻·Ƶusƀ;bd⣒ᤃ⣓戒Ā;uᴼ⣘;横ţ⣞⣡p;櫛ò−ðઁĀdp⣩⣮els;抧f;쀀𝕞Āct⣸⣽r;쀀𝓂pos»ᖝƀ;lm⤉⤊⤍䎼timap;抸ఀGLRVabcdefghijlmoprstuvw⥂⥓⥾⦉⦘⧚⧩⨕⨚⩘⩝⪃⪕⪤⪨⬄⬇⭄⭿⮮ⰴⱧⱼ⳩Āgt⥇⥋;쀀⋙̸Ā;v⥐௏쀀≫⃒ƀelt⥚⥲⥶ftĀar⥡⥧rrow;懍ightarrow;懎;쀀⋘̸Ā;v⥻ే쀀≪⃒ightarrow;懏ĀDd⦎⦓ash;抯ash;抮ʀbcnpt⦣⦧⦬⦱⧌la»˞ute;䅄g;쀀∠⃒ʀ;Eiop඄⦼⧀⧅⧈;쀀⩰̸d;쀀≋̸s;䅉roø඄urĀ;a⧓⧔普lĀ;s⧓ସǳ⧟\0⧣p肻\xA0ଷmpĀ;e௹ఀʀaeouy⧴⧾⨃⨐⨓ǰ⧹\0⧻;橃on;䅈dil;䅆ngĀ;dൾ⨊ot;쀀⩭̸p;橂;䐽ash;怓΀;Aadqsxஒ⨩⨭⨻⩁⩅⩐rr;懗rĀhr⨳⨶k;椤Ā;oᏲᏰot;쀀≐̸uiöୣĀei⩊⩎ar;椨í஘istĀ;s஠டr;쀀𝔫ȀEest௅⩦⩹⩼ƀ;qs஼⩭௡ƀ;qs஼௅⩴lanô௢ií௪Ā;rஶ⪁»ஷƀAap⪊⪍⪑rò⥱rr;憮ar;櫲ƀ;svྍ⪜ྌĀ;d⪡⪢拼;拺cy;䑚΀AEadest⪷⪺⪾⫂⫅⫶⫹rò⥦;쀀≦̸rr;憚r;急Ȁ;fqs఻⫎⫣⫯tĀar⫔⫙rro÷⫁ightarro÷⪐ƀ;qs఻⪺⫪lanôౕĀ;sౕ⫴»శiíౝĀ;rవ⫾iĀ;eచథiäඐĀpt⬌⬑f;쀀𝕟膀¬;in⬙⬚⬶䂬nȀ;Edvஉ⬤⬨⬮;쀀⋹̸ot;쀀⋵̸ǡஉ⬳⬵;拷;拶iĀ;vಸ⬼ǡಸ⭁⭃;拾;拽ƀaor⭋⭣⭩rȀ;ast୻⭕⭚⭟lleì୻l;쀀⫽⃥;쀀∂̸lint;樔ƀ;ceಒ⭰⭳uåಥĀ;cಘ⭸Ā;eಒ⭽ñಘȀAait⮈⮋⮝⮧rò⦈rrƀ;cw⮔⮕⮙憛;쀀⤳̸;쀀↝̸ghtarrow»⮕riĀ;eೋೖ΀chimpqu⮽⯍⯙⬄୸⯤⯯Ȁ;cerല⯆ഷ⯉uå൅;쀀𝓃ortɭ⬅\0\0⯖ará⭖mĀ;e൮⯟Ā;q൴൳suĀbp⯫⯭å೸åഋƀbcp⯶ⰑⰙȀ;Ees⯿ⰀഢⰄ抄;쀀⫅̸etĀ;eഛⰋqĀ;qണⰀcĀ;eലⰗñസȀ;EesⰢⰣൟⰧ抅;쀀⫆̸etĀ;e൘ⰮqĀ;qൠⰣȀgilrⰽⰿⱅⱇìௗlde耻ñ䃱çృiangleĀlrⱒⱜeftĀ;eచⱚñదightĀ;eೋⱥñ೗Ā;mⱬⱭ䎽ƀ;esⱴⱵⱹ䀣ro;愖p;怇ҀDHadgilrsⲏⲔⲙⲞⲣⲰⲶⳓⳣash;抭arr;椄p;쀀≍⃒ash;抬ĀetⲨⲬ;쀀≥⃒;쀀>⃒nfin;槞ƀAetⲽⳁⳅrr;椂;쀀≤⃒Ā;rⳊⳍ쀀<⃒ie;쀀⊴⃒ĀAtⳘⳜrr;椃rie;쀀⊵⃒im;쀀∼⃒ƀAan⳰⳴ⴂrr;懖rĀhr⳺⳽k;椣Ā;oᏧᏥear;椧ቓ᪕\0\0\0\0\0\0\0\0\0\0\0\0\0ⴭ\0ⴸⵈⵠⵥ⵲ⶄᬇ\0\0ⶍⶫ\0ⷈⷎ\0ⷜ⸙⸫⸾⹃Ācsⴱ᪗ute耻ó䃳ĀiyⴼⵅrĀ;c᪞ⵂ耻ô䃴;䐾ʀabios᪠ⵒⵗǈⵚlac;䅑v;樸old;榼lig;䅓Ācr⵩⵭ir;榿;쀀𝔬ͯ⵹\0\0⵼\0ⶂn;䋛ave耻ò䃲;槁Ābmⶈ෴ar;榵Ȁacitⶕ⶘ⶥⶨrò᪀Āir⶝ⶠr;榾oss;榻nå๒;槀ƀaeiⶱⶵⶹcr;䅍ga;䏉ƀcdnⷀⷅǍron;䎿;榶pf;쀀𝕠ƀaelⷔ⷗ǒr;榷rp;榹΀;adiosvⷪⷫⷮ⸈⸍⸐⸖戨rò᪆Ȁ;efmⷷⷸ⸂⸅橝rĀ;oⷾⷿ愴f»ⷿ耻ª䂪耻º䂺gof;抶r;橖lope;橗;橛ƀclo⸟⸡⸧ò⸁ash耻ø䃸l;折iŬⸯ⸴de耻õ䃵esĀ;aǛ⸺s;樶ml耻ö䃶bar;挽ૡ⹞\0⹽\0⺀⺝\0⺢⺹\0\0⻋ຜ\0⼓\0\0⼫⾼\0⿈rȀ;astЃ⹧⹲຅脀¶;l⹭⹮䂶leìЃɩ⹸\0\0⹻m;櫳;櫽y;䐿rʀcimpt⺋⺏⺓ᡥ⺗nt;䀥od;䀮il;怰enk;怱r;쀀𝔭ƀimo⺨⺰⺴Ā;v⺭⺮䏆;䏕maô੶ne;明ƀ;tv⺿⻀⻈䏀chfork»´;䏖Āau⻏⻟nĀck⻕⻝kĀ;h⇴⻛;愎ö⇴sҀ;abcdemst⻳⻴ᤈ⻹⻽⼄⼆⼊⼎䀫cir;樣ir;樢Āouᵀ⼂;樥;橲n肻±ຝim;樦wo;樧ƀipu⼙⼠⼥ntint;樕f;쀀𝕡nd耻£䂣Ԁ;Eaceinosu່⼿⽁⽄⽇⾁⾉⾒⽾⾶;檳p;檷uå໙Ā;c໎⽌̀;acens່⽙⽟⽦⽨⽾pproø⽃urlyeñ໙ñ໎ƀaes⽯⽶⽺pprox;檹qq;檵im;拨iíໟmeĀ;s⾈ຮ怲ƀEas⽸⾐⽺ð⽵ƀdfp໬⾙⾯ƀals⾠⾥⾪lar;挮ine;挒urf;挓Ā;t໻⾴ï໻rel;抰Āci⿀⿅r;쀀𝓅;䏈ncsp;怈̀fiopsu⿚⋢⿟⿥⿫⿱r;쀀𝔮pf;쀀𝕢rime;恗cr;쀀𝓆ƀaeo⿸〉〓tĀei⿾々rnionóڰnt;樖stĀ;e【】䀿ñἙô༔઀ABHabcdefhilmnoprstux぀けさすムㄎㄫㅇㅢㅲㆎ㈆㈕㈤㈩㉘㉮㉲㊐㊰㊷ƀartぇおがròႳòϝail;検aròᱥar;楤΀cdenqrtとふへみわゔヌĀeuねぱ;쀀∽̱te;䅕iãᅮmptyv;榳gȀ;del࿑らるろ;榒;榥å࿑uo耻»䂻rր;abcfhlpstw࿜ガクシスゼゾダッデナp;極Ā;f࿠ゴs;椠;椳s;椞ë≝ð✮l;楅im;楴l;憣;憝Āaiパフil;椚oĀ;nホボ戶aló༞ƀabrョリヮrò៥rk;杳ĀakンヽcĀekヹ・;䁽;䁝Āes㄂㄄;榌lĀduㄊㄌ;榎;榐Ȁaeuyㄗㄜㄧㄩron;䅙Ādiㄡㄥil;䅗ì࿲âヺ;䑀Ȁclqsㄴㄷㄽㅄa;椷dhar;楩uoĀ;rȎȍh;憳ƀacgㅎㅟངlȀ;ipsླྀㅘㅛႜnåႻarôྩt;断ƀilrㅩဣㅮsht;楽;쀀𝔯ĀaoㅷㆆrĀduㅽㅿ»ѻĀ;l႑ㆄ;楬Ā;vㆋㆌ䏁;䏱ƀgns㆕ㇹㇼht̀ahlrstㆤㆰ㇂㇘㇤㇮rrowĀ;t࿜ㆭaéトarpoonĀduㆻㆿowîㅾp»႒eftĀah㇊㇐rrowó࿪arpoonóՑightarrows;應quigarro÷ニhreetimes;拌g;䋚ingdotseñἲƀahm㈍㈐㈓rò࿪aòՑ;怏oustĀ;a㈞㈟掱che»㈟mid;櫮Ȁabpt㈲㈽㉀㉒Ānr㈷㈺g;柭r;懾rëဃƀafl㉇㉊㉎r;榆;쀀𝕣us;樮imes;樵Āap㉝㉧rĀ;g㉣㉤䀩t;榔olint;樒arò㇣Ȁachq㉻㊀Ⴜ㊅quo;怺r;쀀𝓇Ābu・㊊oĀ;rȔȓƀhir㊗㊛㊠reåㇸmes;拊iȀ;efl㊪ၙᠡ㊫方tri;槎luhar;楨;愞ൡ㋕㋛㋟㌬㌸㍱\0㍺㎤\0\0㏬㏰\0㐨㑈㑚㒭㒱㓊㓱\0㘖\0\0㘳cute;䅛quï➺Ԁ;Eaceinpsyᇭ㋳㋵㋿㌂㌋㌏㌟㌦㌩;檴ǰ㋺\0㋼;檸on;䅡uåᇾĀ;dᇳ㌇il;䅟rc;䅝ƀEas㌖㌘㌛;檶p;檺im;择olint;樓iíሄ;䑁otƀ;be㌴ᵇ㌵担;橦΀Aacmstx㍆㍊㍗㍛㍞㍣㍭rr;懘rĀhr㍐㍒ë∨Ā;oਸ਼਴t耻§䂧i;䀻war;椩mĀin㍩ðnuóñt;朶rĀ;o㍶⁕쀀𝔰Ȁacoy㎂㎆㎑㎠rp;景Āhy㎋㎏cy;䑉;䑈rtɭ㎙\0\0㎜iäᑤaraì⹯耻­䂭Āgm㎨㎴maƀ;fv㎱㎲㎲䏃;䏂Ѐ;deglnprካ㏅㏉㏎㏖㏞㏡㏦ot;橪Ā;q኱ኰĀ;E㏓㏔檞;檠Ā;E㏛㏜檝;檟e;扆lus;樤arr;楲aròᄽȀaeit㏸㐈㐏㐗Āls㏽㐄lsetmé㍪hp;樳parsl;槤Ādlᑣ㐔e;挣Ā;e㐜㐝檪Ā;s㐢㐣檬;쀀⪬︀ƀflp㐮㐳㑂tcy;䑌Ā;b㐸㐹䀯Ā;a㐾㐿槄r;挿f;쀀𝕤aĀdr㑍ЂesĀ;u㑔㑕晠it»㑕ƀcsu㑠㑹㒟Āau㑥㑯pĀ;sᆈ㑫;쀀⊓︀pĀ;sᆴ㑵;쀀⊔︀uĀbp㑿㒏ƀ;esᆗᆜ㒆etĀ;eᆗ㒍ñᆝƀ;esᆨᆭ㒖etĀ;eᆨ㒝ñᆮƀ;afᅻ㒦ְrť㒫ֱ»ᅼaròᅈȀcemt㒹㒾㓂㓅r;쀀𝓈tmîñiì㐕aræᆾĀar㓎㓕rĀ;f㓔ឿ昆Āan㓚㓭ightĀep㓣㓪psiloîỠhé⺯s»⡒ʀbcmnp㓻㕞ሉ㖋㖎Ҁ;Edemnprs㔎㔏㔑㔕㔞㔣㔬㔱㔶抂;櫅ot;檽Ā;dᇚ㔚ot;櫃ult;櫁ĀEe㔨㔪;櫋;把lus;檿arr;楹ƀeiu㔽㕒㕕tƀ;en㔎㕅㕋qĀ;qᇚ㔏eqĀ;q㔫㔨m;櫇Ābp㕚㕜;櫕;櫓c̀;acensᇭ㕬㕲㕹㕻㌦pproø㋺urlyeñᇾñᇳƀaes㖂㖈㌛pproø㌚qñ㌗g;晪ڀ123;Edehlmnps㖩㖬㖯ሜ㖲㖴㗀㗉㗕㗚㗟㗨㗭耻¹䂹耻²䂲耻³䂳;櫆Āos㖹㖼t;檾ub;櫘Ā;dሢ㗅ot;櫄sĀou㗏㗒l;柉b;櫗arr;楻ult;櫂ĀEe㗤㗦;櫌;抋lus;櫀ƀeiu㗴㘉㘌tƀ;enሜ㗼㘂qĀ;qሢ㖲eqĀ;q㗧㗤m;櫈Ābp㘑㘓;櫔;櫖ƀAan㘜㘠㘭rr;懙rĀhr㘦㘨ë∮Ā;oਫ਩war;椪lig耻ß䃟௡㙑㙝㙠ዎ㙳㙹\0㙾㛂\0\0\0\0\0㛛㜃\0㜉㝬\0\0\0㞇ɲ㙖\0\0㙛get;挖;䏄rë๟ƀaey㙦㙫㙰ron;䅥dil;䅣;䑂lrec;挕r;쀀𝔱Ȁeiko㚆㚝㚵㚼ǲ㚋\0㚑eĀ4fኄኁaƀ;sv㚘㚙㚛䎸ym;䏑Ācn㚢㚲kĀas㚨㚮pproø዁im»ኬsðኞĀas㚺㚮ð዁rn耻þ䃾Ǭ̟㛆⋧es膀×;bd㛏㛐㛘䃗Ā;aᤏ㛕r;樱;樰ƀeps㛡㛣㜀á⩍Ȁ;bcf҆㛬㛰㛴ot;挶ir;櫱Ā;o㛹㛼쀀𝕥rk;櫚á㍢rime;怴ƀaip㜏㜒㝤dåቈ΀adempst㜡㝍㝀㝑㝗㝜㝟ngleʀ;dlqr㜰㜱㜶㝀㝂斵own»ᶻeftĀ;e⠀㜾ñम;扜ightĀ;e㊪㝋ñၚot;旬inus;樺lus;樹b;槍ime;樻ezium;揢ƀcht㝲㝽㞁Āry㝷㝻;쀀𝓉;䑆cy;䑛rok;䅧Āio㞋㞎xô᝷headĀlr㞗㞠eftarro÷ࡏightarrow»ཝऀAHabcdfghlmoprstuw㟐㟓㟗㟤㟰㟼㠎㠜㠣㠴㡑㡝㡫㢩㣌㣒㣪㣶ròϭar;楣Ācr㟜㟢ute耻ú䃺òᅐrǣ㟪\0㟭y;䑞ve;䅭Āiy㟵㟺rc耻û䃻;䑃ƀabh㠃㠆㠋ròᎭlac;䅱aòᏃĀir㠓㠘sht;楾;쀀𝔲rave耻ù䃹š㠧㠱rĀlr㠬㠮»ॗ»ႃlk;斀Āct㠹㡍ɯ㠿\0\0㡊rnĀ;e㡅㡆挜r»㡆op;挏ri;旸Āal㡖㡚cr;䅫肻¨͉Āgp㡢㡦on;䅳f;쀀𝕦̀adhlsuᅋ㡸㡽፲㢑㢠ownáᎳarpoonĀlr㢈㢌efô㠭ighô㠯iƀ;hl㢙㢚㢜䏅»ᏺon»㢚parrows;懈ƀcit㢰㣄㣈ɯ㢶\0\0㣁rnĀ;e㢼㢽挝r»㢽op;挎ng;䅯ri;旹cr;쀀𝓊ƀdir㣙㣝㣢ot;拰lde;䅩iĀ;f㜰㣨»᠓Āam㣯㣲rò㢨l耻ü䃼angle;榧ހABDacdeflnoprsz㤜㤟㤩㤭㦵㦸㦽㧟㧤㧨㧳㧹㧽㨁㨠ròϷarĀ;v㤦㤧櫨;櫩asèϡĀnr㤲㤷grt;榜΀eknprst㓣㥆㥋㥒㥝㥤㦖appá␕othinçẖƀhir㓫⻈㥙opô⾵Ā;hᎷ㥢ïㆍĀiu㥩㥭gmá㎳Ābp㥲㦄setneqĀ;q㥽㦀쀀⊊︀;쀀⫋︀setneqĀ;q㦏㦒쀀⊋︀;쀀⫌︀Āhr㦛㦟etá㚜iangleĀlr㦪㦯eft»थight»ၑy;䐲ash»ံƀelr㧄㧒㧗ƀ;beⷪ㧋㧏ar;抻q;扚lip;拮Ābt㧜ᑨaòᑩr;쀀𝔳tré㦮suĀbp㧯㧱»ജ»൙pf;쀀𝕧roð໻tré㦴Ācu㨆㨋r;쀀𝓋Ābp㨐㨘nĀEe㦀㨖»㥾nĀEe㦒㨞»㦐igzag;榚΀cefoprs㨶㨻㩖㩛㩔㩡㩪irc;䅵Ādi㩀㩑Ābg㩅㩉ar;機eĀ;qᗺ㩏;扙erp;愘r;쀀𝔴pf;쀀𝕨Ā;eᑹ㩦atèᑹcr;쀀𝓌ૣណ㪇\0㪋\0㪐㪛\0\0㪝㪨㪫㪯\0\0㫃㫎\0㫘ៜ៟tré៑r;쀀𝔵ĀAa㪔㪗ròσrò৶;䎾ĀAa㪡㪤ròθrò৫að✓is;拻ƀdptឤ㪵㪾Āfl㪺ឩ;쀀𝕩imåឲĀAa㫇㫊ròώròਁĀcq㫒ីr;쀀𝓍Āpt៖㫜ré។Ѐacefiosu㫰㫽㬈㬌㬑㬕㬛㬡cĀuy㫶㫻te耻ý䃽;䑏Āiy㬂㬆rc;䅷;䑋n耻¥䂥r;쀀𝔶cy;䑗pf;쀀𝕪cr;쀀𝓎Ācm㬦㬩y;䑎l耻ÿ䃿Ԁacdefhiosw㭂㭈㭔㭘㭤㭩㭭㭴㭺㮀cute;䅺Āay㭍㭒ron;䅾;䐷ot;䅼Āet㭝㭡træᕟa;䎶r;쀀𝔷cy;䐶grarr;懝pf;쀀𝕫cr;쀀𝓏Ājn㮅㮇;怍j;怌".split("").map((e) => e.charCodeAt(0))), ht = new Uint16Array("Ȁaglq	\x1Bɭ\0\0p;䀦os;䀧t;䀾t;䀼uot;䀢".split("").map((e) => e.charCodeAt(0))), gt = /* @__PURE__ */ new Map([
	[0, 65533],
	[128, 8364],
	[130, 8218],
	[131, 402],
	[132, 8222],
	[133, 8230],
	[134, 8224],
	[135, 8225],
	[136, 710],
	[137, 8240],
	[138, 352],
	[139, 8249],
	[140, 338],
	[142, 381],
	[145, 8216],
	[146, 8217],
	[147, 8220],
	[148, 8221],
	[149, 8226],
	[150, 8211],
	[151, 8212],
	[152, 732],
	[153, 8482],
	[154, 353],
	[155, 8250],
	[156, 339],
	[158, 382],
	[159, 376]
]), _t = String.fromCodePoint ?? function(e) {
	let t = "";
	return e > 65535 && (e -= 65536, t += String.fromCharCode(e >>> 10 & 1023 | 55296), e = 56320 | e & 1023), t += String.fromCharCode(e), t;
};
function vt(e) {
	return e >= 55296 && e <= 57343 || e > 1114111 ? 65533 : gt.get(e) ?? e;
}
//#endregion
//#region ../../../node_modules/entities/lib/esm/decode.js
var N;
(function(e) {
	e[e.NUM = 35] = "NUM", e[e.SEMI = 59] = "SEMI", e[e.EQUALS = 61] = "EQUALS", e[e.ZERO = 48] = "ZERO", e[e.NINE = 57] = "NINE", e[e.LOWER_A = 97] = "LOWER_A", e[e.LOWER_F = 102] = "LOWER_F", e[e.LOWER_X = 120] = "LOWER_X", e[e.LOWER_Z = 122] = "LOWER_Z", e[e.UPPER_A = 65] = "UPPER_A", e[e.UPPER_F = 70] = "UPPER_F", e[e.UPPER_Z = 90] = "UPPER_Z";
})(N ||= {});
var yt = 32, P;
(function(e) {
	e[e.VALUE_LENGTH = 49152] = "VALUE_LENGTH", e[e.BRANCH_LENGTH = 16256] = "BRANCH_LENGTH", e[e.JUMP_TABLE = 127] = "JUMP_TABLE";
})(P ||= {});
function F(e) {
	return e >= N.ZERO && e <= N.NINE;
}
function bt(e) {
	return e >= N.UPPER_A && e <= N.UPPER_F || e >= N.LOWER_A && e <= N.LOWER_F;
}
function xt(e) {
	return e >= N.UPPER_A && e <= N.UPPER_Z || e >= N.LOWER_A && e <= N.LOWER_Z || F(e);
}
function St(e) {
	return e === N.EQUALS || xt(e);
}
var I;
(function(e) {
	e[e.EntityStart = 0] = "EntityStart", e[e.NumericStart = 1] = "NumericStart", e[e.NumericDecimal = 2] = "NumericDecimal", e[e.NumericHex = 3] = "NumericHex", e[e.NamedEntity = 4] = "NamedEntity";
})(I ||= {});
var L;
(function(e) {
	e[e.Legacy = 0] = "Legacy", e[e.Strict = 1] = "Strict", e[e.Attribute = 2] = "Attribute";
})(L ||= {});
var Ct = class {
	constructor(e, t, n) {
		this.decodeTree = e, this.emitCodePoint = t, this.errors = n, this.state = I.EntityStart, this.consumed = 1, this.result = 0, this.treeIndex = 0, this.excess = 1, this.decodeMode = L.Strict;
	}
	startEntity(e) {
		this.decodeMode = e, this.state = I.EntityStart, this.result = 0, this.treeIndex = 0, this.excess = 1, this.consumed = 1;
	}
	write(e, t) {
		switch (this.state) {
			case I.EntityStart: return e.charCodeAt(t) === N.NUM ? (this.state = I.NumericStart, this.consumed += 1, this.stateNumericStart(e, t + 1)) : (this.state = I.NamedEntity, this.stateNamedEntity(e, t));
			case I.NumericStart: return this.stateNumericStart(e, t);
			case I.NumericDecimal: return this.stateNumericDecimal(e, t);
			case I.NumericHex: return this.stateNumericHex(e, t);
			case I.NamedEntity: return this.stateNamedEntity(e, t);
		}
	}
	stateNumericStart(e, t) {
		return t >= e.length ? -1 : (e.charCodeAt(t) | yt) === N.LOWER_X ? (this.state = I.NumericHex, this.consumed += 1, this.stateNumericHex(e, t + 1)) : (this.state = I.NumericDecimal, this.stateNumericDecimal(e, t));
	}
	addToNumericResult(e, t, n, r) {
		if (t !== n) {
			let i = n - t;
			this.result = this.result * r ** +i + parseInt(e.substr(t, i), r), this.consumed += i;
		}
	}
	stateNumericHex(e, t) {
		let n = t;
		for (; t < e.length;) {
			let r = e.charCodeAt(t);
			if (F(r) || bt(r)) t += 1;
			else return this.addToNumericResult(e, n, t, 16), this.emitNumericEntity(r, 3);
		}
		return this.addToNumericResult(e, n, t, 16), -1;
	}
	stateNumericDecimal(e, t) {
		let n = t;
		for (; t < e.length;) {
			let r = e.charCodeAt(t);
			if (F(r)) t += 1;
			else return this.addToNumericResult(e, n, t, 10), this.emitNumericEntity(r, 2);
		}
		return this.addToNumericResult(e, n, t, 10), -1;
	}
	emitNumericEntity(e, t) {
		var n;
		if (this.consumed <= t) return (n = this.errors) == null || n.absenceOfDigitsInNumericCharacterReference(this.consumed), 0;
		if (e === N.SEMI) this.consumed += 1;
		else if (this.decodeMode === L.Strict) return 0;
		return this.emitCodePoint(vt(this.result), this.consumed), this.errors && (e !== N.SEMI && this.errors.missingSemicolonAfterCharacterReference(), this.errors.validateNumericCharacterReference(this.result)), this.consumed;
	}
	stateNamedEntity(e, t) {
		let { decodeTree: n } = this, r = n[this.treeIndex], i = (r & P.VALUE_LENGTH) >> 14;
		for (; t < e.length; t++, this.excess++) {
			let a = e.charCodeAt(t);
			if (this.treeIndex = Tt(n, r, this.treeIndex + Math.max(1, i), a), this.treeIndex < 0) return this.result === 0 || this.decodeMode === L.Attribute && (i === 0 || St(a)) ? 0 : this.emitNotTerminatedNamedEntity();
			if (r = n[this.treeIndex], i = (r & P.VALUE_LENGTH) >> 14, i !== 0) {
				if (a === N.SEMI) return this.emitNamedEntityData(this.treeIndex, i, this.consumed + this.excess);
				this.decodeMode !== L.Strict && (this.result = this.treeIndex, this.consumed += this.excess, this.excess = 0);
			}
		}
		return -1;
	}
	emitNotTerminatedNamedEntity() {
		var e;
		let { result: t, decodeTree: n } = this, r = (n[t] & P.VALUE_LENGTH) >> 14;
		return this.emitNamedEntityData(t, r, this.consumed), (e = this.errors) == null || e.missingSemicolonAfterCharacterReference(), this.consumed;
	}
	emitNamedEntityData(e, t, n) {
		let { decodeTree: r } = this;
		return this.emitCodePoint(t === 1 ? r[e] & ~P.VALUE_LENGTH : r[e + 1], n), t === 3 && this.emitCodePoint(r[e + 2], n), n;
	}
	end() {
		var e;
		switch (this.state) {
			case I.NamedEntity: return this.result !== 0 && (this.decodeMode !== L.Attribute || this.result === this.treeIndex) ? this.emitNotTerminatedNamedEntity() : 0;
			case I.NumericDecimal: return this.emitNumericEntity(0, 2);
			case I.NumericHex: return this.emitNumericEntity(0, 3);
			case I.NumericStart: return (e = this.errors) == null || e.absenceOfDigitsInNumericCharacterReference(this.consumed), 0;
			case I.EntityStart: return 0;
		}
	}
};
function wt(e) {
	let t = "", n = new Ct(e, (e) => t += _t(e));
	return function(e, r) {
		let i = 0, a = 0;
		for (; (a = e.indexOf("&", a)) >= 0;) {
			t += e.slice(i, a), n.startEntity(r);
			let o = n.write(e, a + 1);
			if (o < 0) {
				i = a + n.end();
				break;
			}
			i = a + o, a = o === 0 ? i + 1 : i;
		}
		let o = t + e.slice(i);
		return t = "", o;
	};
}
function Tt(e, t, n, r) {
	let i = (t & P.BRANCH_LENGTH) >> 7, a = t & P.JUMP_TABLE;
	if (i === 0) return a !== 0 && r === a ? n : -1;
	if (a) {
		let t = r - a;
		return t < 0 || t >= i ? -1 : e[n + t] - 1;
	}
	let o = n, s = o + i - 1;
	for (; o <= s;) {
		let t = o + s >>> 1, n = e[t];
		if (n < r) o = t + 1;
		else if (n > r) s = t - 1;
		else return e[t + i];
	}
	return -1;
}
var R = wt(mt);
wt(ht);
function z(e, t = L.Legacy) {
	return R(e, t);
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/common/utils.mjs
var Et = /* @__PURE__ */ t({
	arrayReplaceAt: () => B,
	assign: () => jt,
	escapeHtml: () => Vt,
	escapeRE: () => Ut,
	fromCodePoint: () => Mt,
	has: () => At,
	isMdAsciiPunct: () => Kt,
	isPunctChar: () => Gt,
	isSpace: () => W,
	isString: () => Ot,
	isValidEntityCode: () => V,
	isWhiteSpace: () => Wt,
	lib: () => Jt,
	normalizeReference: () => qt,
	unescapeAll: () => U,
	unescapeMd: () => It
});
function Dt(e) {
	return Object.prototype.toString.call(e);
}
function Ot(e) {
	return Dt(e) === "[object String]";
}
var kt = Object.prototype.hasOwnProperty;
function At(e, t) {
	return kt.call(e, t);
}
function jt(e) {
	return Array.prototype.slice.call(arguments, 1).forEach(function(t) {
		if (t) {
			if (typeof t != "object") throw TypeError(t + "must be object");
			Object.keys(t).forEach(function(n) {
				e[n] = t[n];
			});
		}
	}), e;
}
function B(e, t, n) {
	return [].concat(e.slice(0, t), n, e.slice(t + 1));
}
function V(e) {
	return !(e >= 55296 && e <= 57343 || e >= 64976 && e <= 65007 || (e & 65535) == 65535 || (e & 65535) == 65534 || e >= 0 && e <= 8 || e === 11 || e >= 14 && e <= 31 || e >= 127 && e <= 159 || e > 1114111);
}
function Mt(e) {
	if (e > 65535) {
		e -= 65536;
		let t = 55296 + (e >> 10), n = 56320 + (e & 1023);
		return String.fromCharCode(t, n);
	}
	return String.fromCharCode(e);
}
var Nt = /\\([!"#$%&'()*+,\-./:;<=>?@[\\\]^_`{|}~])/g, Pt = RegExp(Nt.source + "|&([a-z#][a-z0-9]{1,31});", "gi"), Ft = /^#((?:x[a-f0-9]{1,8}|[0-9]{1,8}))$/i;
function H(e, t) {
	if (t.charCodeAt(0) === 35 && Ft.test(t)) {
		let n = t[1].toLowerCase() === "x" ? parseInt(t.slice(2), 16) : parseInt(t.slice(1), 10);
		return V(n) ? Mt(n) : e;
	}
	let n = z(e);
	return n === e ? e : n;
}
function It(e) {
	return e.indexOf("\\") < 0 ? e : e.replace(Nt, "$1");
}
function U(e) {
	return e.indexOf("\\") < 0 && e.indexOf("&") < 0 ? e : e.replace(Pt, function(e, t, n) {
		return t || H(e, n);
	});
}
var Lt = /[&<>"]/, Rt = /[&<>"]/g, zt = {
	"&": "&amp;",
	"<": "&lt;",
	">": "&gt;",
	"\"": "&quot;"
};
function Bt(e) {
	return zt[e];
}
function Vt(e) {
	return Lt.test(e) ? e.replace(Rt, Bt) : e;
}
var Ht = /[.?*+^$[\]\\(){}|-]/g;
function Ut(e) {
	return e.replace(Ht, "\\$&");
}
function W(e) {
	switch (e) {
		case 9:
		case 32: return !0;
	}
	return !1;
}
function Wt(e) {
	if (e >= 8192 && e <= 8202) return !0;
	switch (e) {
		case 9:
		case 10:
		case 11:
		case 12:
		case 13:
		case 32:
		case 160:
		case 5760:
		case 8239:
		case 8287:
		case 12288: return !0;
	}
	return !1;
}
function Gt(e) {
	return ut.test(e) || dt.test(e);
}
function Kt(e) {
	switch (e) {
		case 33:
		case 34:
		case 35:
		case 36:
		case 37:
		case 38:
		case 39:
		case 40:
		case 41:
		case 42:
		case 43:
		case 44:
		case 45:
		case 46:
		case 47:
		case 58:
		case 59:
		case 60:
		case 61:
		case 62:
		case 63:
		case 64:
		case 91:
		case 92:
		case 93:
		case 94:
		case 95:
		case 96:
		case 123:
		case 124:
		case 125:
		case 126: return !0;
		default: return !1;
	}
}
function qt(e) {
	return e = e.trim().replace(/\s+/g, " "), e.toLowerCase().toUpperCase();
}
var Jt = {
	mdurl: ot,
	ucmicro: pt
};
//#endregion
//#region ../../../node_modules/markdown-it/lib/helpers/parse_link_label.mjs
function Yt(e, t, n) {
	let r, i, a, o, s = e.posMax, c = e.pos;
	for (e.pos = t + 1, r = 1; e.pos < s;) {
		if (a = e.src.charCodeAt(e.pos), a === 93 && (r--, r === 0)) {
			i = !0;
			break;
		}
		if (o = e.pos, e.md.inline.skipToken(e), a === 91) {
			if (o === e.pos - 1) r++;
			else if (n) return e.pos = c, -1;
		}
	}
	let l = -1;
	return i && (l = e.pos), e.pos = c, l;
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/helpers/parse_link_destination.mjs
function Xt(e, t, n) {
	let r, i = t, a = {
		ok: !1,
		pos: 0,
		str: ""
	};
	if (e.charCodeAt(i) === 60) {
		for (i++; i < n;) {
			if (r = e.charCodeAt(i), r === 10 || r === 60) return a;
			if (r === 62) return a.pos = i + 1, a.str = U(e.slice(t + 1, i)), a.ok = !0, a;
			if (r === 92 && i + 1 < n) {
				i += 2;
				continue;
			}
			i++;
		}
		return a;
	}
	let o = 0;
	for (; i < n && (r = e.charCodeAt(i), !(r === 32 || r < 32 || r === 127));) {
		if (r === 92 && i + 1 < n) {
			if (e.charCodeAt(i + 1) === 32) break;
			i += 2;
			continue;
		}
		if (r === 40 && (o++, o > 32)) return a;
		if (r === 41) {
			if (o === 0) break;
			o--;
		}
		i++;
	}
	return t === i || o !== 0 ? a : (a.str = U(e.slice(t, i)), a.pos = i, a.ok = !0, a);
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/helpers/parse_link_title.mjs
function Zt(e, t, n, r) {
	let i, a = t, o = {
		ok: !1,
		can_continue: !1,
		pos: 0,
		str: "",
		marker: 0
	};
	if (r) o.str = r.str, o.marker = r.marker;
	else {
		if (a >= n) return o;
		let r = e.charCodeAt(a);
		if (r !== 34 && r !== 39 && r !== 40) return o;
		t++, a++, r === 40 && (r = 41), o.marker = r;
	}
	for (; a < n;) {
		if (i = e.charCodeAt(a), i === o.marker) return o.pos = a + 1, o.str += U(e.slice(t, a)), o.ok = !0, o;
		if (i === 40 && o.marker === 41) return o;
		i === 92 && a + 1 < n && a++, a++;
	}
	return o.can_continue = !0, o.str += U(e.slice(t, a)), o;
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/helpers/index.mjs
var Qt = /* @__PURE__ */ t({
	parseLinkDestination: () => Xt,
	parseLinkLabel: () => Yt,
	parseLinkTitle: () => Zt
}), G = {};
G.code_inline = function(e, t, n, r, i) {
	let a = e[t];
	return "<code" + i.renderAttrs(a) + ">" + Vt(a.content) + "</code>";
}, G.code_block = function(e, t, n, r, i) {
	let a = e[t];
	return "<pre" + i.renderAttrs(a) + "><code>" + Vt(e[t].content) + "</code></pre>\n";
}, G.fence = function(e, t, n, r, i) {
	let a = e[t], o = a.info ? U(a.info).trim() : "", s = "", c = "";
	if (o) {
		let e = o.split(/(\s+)/g);
		s = e[0], c = e.slice(2).join("");
	}
	let l;
	if (l = n.highlight && n.highlight(a.content, s, c) || Vt(a.content), l.indexOf("<pre") === 0) return l + "\n";
	if (o) {
		let e = a.attrIndex("class"), t = a.attrs ? a.attrs.slice() : [];
		e < 0 ? t.push(["class", n.langPrefix + s]) : (t[e] = t[e].slice(), t[e][1] += " " + n.langPrefix + s);
		let r = { attrs: t };
		return `<pre><code${i.renderAttrs(r)}>${l}</code></pre>\n`;
	}
	return `<pre><code${i.renderAttrs(a)}>${l}</code></pre>\n`;
}, G.image = function(e, t, n, r, i) {
	let a = e[t];
	return a.attrs[a.attrIndex("alt")][1] = i.renderInlineAsText(a.children, n, r), i.renderToken(e, t, n);
}, G.hardbreak = function(e, t, n) {
	return n.xhtmlOut ? "<br />\n" : "<br>\n";
}, G.softbreak = function(e, t, n) {
	return n.breaks ? n.xhtmlOut ? "<br />\n" : "<br>\n" : "\n";
}, G.text = function(e, t) {
	return Vt(e[t].content);
}, G.html_block = function(e, t) {
	return e[t].content;
}, G.html_inline = function(e, t) {
	return e[t].content;
};
function $t() {
	this.rules = jt({}, G);
}
$t.prototype.renderAttrs = function(e) {
	let t, n, r;
	if (!e.attrs) return "";
	for (r = "", t = 0, n = e.attrs.length; t < n; t++) r += " " + Vt(e.attrs[t][0]) + "=\"" + Vt(e.attrs[t][1]) + "\"";
	return r;
}, $t.prototype.renderToken = function(e, t, n) {
	let r = e[t], i = "";
	if (r.hidden) return "";
	r.block && r.nesting !== -1 && t && e[t - 1].hidden && (i += "\n"), i += (r.nesting === -1 ? "</" : "<") + r.tag, i += this.renderAttrs(r), r.nesting === 0 && n.xhtmlOut && (i += " /");
	let a = !1;
	if (r.block && (a = !0, r.nesting === 1 && t + 1 < e.length)) {
		let n = e[t + 1];
		(n.type === "inline" || n.hidden || n.nesting === -1 && n.tag === r.tag) && (a = !1);
	}
	return i += a ? ">\n" : ">", i;
}, $t.prototype.renderInline = function(e, t, n) {
	let r = "", i = this.rules;
	for (let a = 0, o = e.length; a < o; a++) {
		let o = e[a].type;
		i[o] === void 0 ? r += this.renderToken(e, a, t) : r += i[o](e, a, t, n, this);
	}
	return r;
}, $t.prototype.renderInlineAsText = function(e, t, n) {
	let r = "";
	for (let i = 0, a = e.length; i < a; i++) switch (e[i].type) {
		case "text":
			r += e[i].content;
			break;
		case "image":
			r += this.renderInlineAsText(e[i].children, t, n);
			break;
		case "html_inline":
		case "html_block":
			r += e[i].content;
			break;
		case "softbreak":
		case "hardbreak": r += "\n";
	}
	return r;
}, $t.prototype.render = function(e, t, n) {
	let r = "", i = this.rules;
	for (let a = 0, o = e.length; a < o; a++) {
		let o = e[a].type;
		o === "inline" ? r += this.renderInline(e[a].children, t, n) : i[o] === void 0 ? r += this.renderToken(e, a, t, n) : r += i[o](e, a, t, n, this);
	}
	return r;
};
//#endregion
//#region ../../../node_modules/markdown-it/lib/ruler.mjs
function K() {
	this.__rules__ = [], this.__cache__ = null;
}
K.prototype.__find__ = function(e) {
	for (let t = 0; t < this.__rules__.length; t++) if (this.__rules__[t].name === e) return t;
	return -1;
}, K.prototype.__compile__ = function() {
	let e = this, t = [""];
	e.__rules__.forEach(function(e) {
		e.enabled && e.alt.forEach(function(e) {
			t.indexOf(e) < 0 && t.push(e);
		});
	}), e.__cache__ = {}, t.forEach(function(t) {
		e.__cache__[t] = [], e.__rules__.forEach(function(n) {
			n.enabled && (t && n.alt.indexOf(t) < 0 || e.__cache__[t].push(n.fn));
		});
	});
}, K.prototype.at = function(e, t, n) {
	let r = this.__find__(e), i = n || {};
	if (r === -1) throw Error("Parser rule not found: " + e);
	this.__rules__[r].fn = t, this.__rules__[r].alt = i.alt || [], this.__cache__ = null;
}, K.prototype.before = function(e, t, n, r) {
	let i = this.__find__(e), a = r || {};
	if (i === -1) throw Error("Parser rule not found: " + e);
	this.__rules__.splice(i, 0, {
		name: t,
		enabled: !0,
		fn: n,
		alt: a.alt || []
	}), this.__cache__ = null;
}, K.prototype.after = function(e, t, n, r) {
	let i = this.__find__(e), a = r || {};
	if (i === -1) throw Error("Parser rule not found: " + e);
	this.__rules__.splice(i + 1, 0, {
		name: t,
		enabled: !0,
		fn: n,
		alt: a.alt || []
	}), this.__cache__ = null;
}, K.prototype.push = function(e, t, n) {
	let r = n || {};
	this.__rules__.push({
		name: e,
		enabled: !0,
		fn: t,
		alt: r.alt || []
	}), this.__cache__ = null;
}, K.prototype.enable = function(e, t) {
	Array.isArray(e) || (e = [e]);
	let n = [];
	return e.forEach(function(e) {
		let r = this.__find__(e);
		if (r < 0) {
			if (t) return;
			throw Error("Rules manager: invalid rule name " + e);
		}
		this.__rules__[r].enabled = !0, n.push(e);
	}, this), this.__cache__ = null, n;
}, K.prototype.enableOnly = function(e, t) {
	Array.isArray(e) || (e = [e]), this.__rules__.forEach(function(e) {
		e.enabled = !1;
	}), this.enable(e, t);
}, K.prototype.disable = function(e, t) {
	Array.isArray(e) || (e = [e]);
	let n = [];
	return e.forEach(function(e) {
		let r = this.__find__(e);
		if (r < 0) {
			if (t) return;
			throw Error("Rules manager: invalid rule name " + e);
		}
		this.__rules__[r].enabled = !1, n.push(e);
	}, this), this.__cache__ = null, n;
}, K.prototype.getRules = function(e) {
	return this.__cache__ === null && this.__compile__(), this.__cache__[e] || [];
};
//#endregion
//#region ../../../node_modules/markdown-it/lib/token.mjs
function q(e, t, n) {
	this.type = e, this.tag = t, this.attrs = null, this.map = null, this.nesting = n, this.level = 0, this.children = null, this.content = "", this.markup = "", this.info = "", this.meta = null, this.block = !1, this.hidden = !1;
}
q.prototype.attrIndex = function(e) {
	if (!this.attrs) return -1;
	let t = this.attrs;
	for (let n = 0, r = t.length; n < r; n++) if (t[n][0] === e) return n;
	return -1;
}, q.prototype.attrPush = function(e) {
	this.attrs ? this.attrs.push(e) : this.attrs = [e];
}, q.prototype.attrSet = function(e, t) {
	let n = this.attrIndex(e), r = [e, t];
	n < 0 ? this.attrPush(r) : this.attrs[n] = r;
}, q.prototype.attrGet = function(e) {
	let t = this.attrIndex(e), n = null;
	return t >= 0 && (n = this.attrs[t][1]), n;
}, q.prototype.attrJoin = function(e, t) {
	let n = this.attrIndex(e);
	n < 0 ? this.attrPush([e, t]) : this.attrs[n][1] = this.attrs[n][1] + " " + t;
};
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_core/state_core.mjs
function en(e, t, n) {
	this.src = e, this.env = n, this.tokens = [], this.inlineMode = !1, this.md = t;
}
en.prototype.Token = q;
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_core/normalize.mjs
var tn = /\r\n?|\n/g, nn = /\0/g;
function rn(e) {
	let t;
	t = e.src.replace(tn, "\n"), t = t.replace(nn, "�"), e.src = t;
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_core/block.mjs
function an(e) {
	let t;
	e.inlineMode ? (t = new e.Token("inline", "", 0), t.content = e.src, t.map = [0, 1], t.children = [], e.tokens.push(t)) : e.md.block.parse(e.src, e.md, e.env, e.tokens);
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_core/inline.mjs
function on(e) {
	let t = e.tokens;
	for (let n = 0, r = t.length; n < r; n++) {
		let r = t[n];
		r.type === "inline" && e.md.inline.parse(r.content, e.md, e.env, r.children);
	}
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_core/linkify.mjs
function sn(e) {
	return /^<a[>\s]/i.test(e);
}
function cn(e) {
	return /^<\/a\s*>/i.test(e);
}
function ln(e) {
	let t = e.tokens;
	if (e.md.options.linkify) for (let n = 0, r = t.length; n < r; n++) {
		if (t[n].type !== "inline" || !e.md.linkify.pretest(t[n].content)) continue;
		let r = t[n].children, i = 0;
		for (let a = r.length - 1; a >= 0; a--) {
			let o = r[a];
			if (o.type === "link_close") {
				for (a--; r[a].level !== o.level && r[a].type !== "link_open";) a--;
				continue;
			}
			if (o.type === "html_inline" && (sn(o.content) && i > 0 && i--, cn(o.content) && i++), !(i > 0) && o.type === "text" && e.md.linkify.test(o.content)) {
				let i = o.content, s = e.md.linkify.match(i), c = [], l = o.level, u = 0;
				s.length > 0 && s[0].index === 0 && a > 0 && r[a - 1].type === "text_special" && (s = s.slice(1));
				for (let t = 0; t < s.length; t++) {
					let n = s[t].url, r = e.md.normalizeLink(n);
					if (!e.md.validateLink(r)) continue;
					let a = s[t].text;
					a = s[t].schema ? s[t].schema === "mailto:" && !/^mailto:/i.test(a) ? e.md.normalizeLinkText("mailto:" + a).replace(/^mailto:/, "") : e.md.normalizeLinkText(a) : e.md.normalizeLinkText("http://" + a).replace(/^http:\/\//, "");
					let o = s[t].index;
					if (o > u) {
						let t = new e.Token("text", "", 0);
						t.content = i.slice(u, o), t.level = l, c.push(t);
					}
					let d = new e.Token("link_open", "a", 1);
					d.attrs = [["href", r]], d.level = l++, d.markup = "linkify", d.info = "auto", c.push(d);
					let f = new e.Token("text", "", 0);
					f.content = a, f.level = l, c.push(f);
					let p = new e.Token("link_close", "a", -1);
					p.level = --l, p.markup = "linkify", p.info = "auto", c.push(p), u = s[t].lastIndex;
				}
				if (u < i.length) {
					let t = new e.Token("text", "", 0);
					t.content = i.slice(u), t.level = l, c.push(t);
				}
				t[n].children = r = B(r, a, c);
			}
		}
	}
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_core/replacements.mjs
var un = /\+-|\.\.|\?\?\?\?|!!!!|,,|--/, dn = /\((c|tm|r)\)/i, fn = /\((c|tm|r)\)/gi, pn = {
	c: "©",
	r: "®",
	tm: "™"
};
function mn(e, t) {
	return pn[t.toLowerCase()];
}
function hn(e) {
	let t = 0;
	for (let n = e.length - 1; n >= 0; n--) {
		let r = e[n];
		r.type === "text" && !t && (r.content = r.content.replace(fn, mn)), r.type === "link_open" && r.info === "auto" && t--, r.type === "link_close" && r.info === "auto" && t++;
	}
}
function gn(e) {
	let t = 0;
	for (let n = e.length - 1; n >= 0; n--) {
		let r = e[n];
		r.type === "text" && !t && un.test(r.content) && (r.content = r.content.replace(/\+-/g, "±").replace(/\.{2,}/g, "…").replace(/([?!])…/g, "$1..").replace(/([?!]){4,}/g, "$1$1$1").replace(/,{2,}/g, ",").replace(/(^|[^-])---(?=[^-]|$)/gm, "$1—").replace(/(^|\s)--(?=\s|$)/gm, "$1–").replace(/(^|[^-\s])--(?=[^-\s]|$)/gm, "$1–")), r.type === "link_open" && r.info === "auto" && t--, r.type === "link_close" && r.info === "auto" && t++;
	}
}
function _n(e) {
	let t;
	if (e.md.options.typographer) for (t = e.tokens.length - 1; t >= 0; t--) e.tokens[t].type === "inline" && (dn.test(e.tokens[t].content) && hn(e.tokens[t].children), un.test(e.tokens[t].content) && gn(e.tokens[t].children));
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_core/smartquotes.mjs
var vn = /['"]/, yn = /['"]/g, bn = "’";
function xn(e, t, n) {
	return e.slice(0, t) + n + e.slice(t + 1);
}
function Sn(e, t) {
	let n, r = [];
	for (let i = 0; i < e.length; i++) {
		let a = e[i], o = e[i].level;
		for (n = r.length - 1; n >= 0 && !(r[n].level <= o); n--);
		if (r.length = n + 1, a.type !== "text") continue;
		let s = a.content, c = 0, l = s.length;
		OUTER: for (; c < l;) {
			yn.lastIndex = c;
			let u = yn.exec(s);
			if (!u) break;
			let d = !0, f = !0;
			c = u.index + 1;
			let p = u[0] === "'", m = 32;
			if (u.index - 1 >= 0) m = s.charCodeAt(u.index - 1);
			else for (n = i - 1; n >= 0 && e[n].type !== "softbreak" && e[n].type !== "hardbreak"; n--) if (e[n].content) {
				m = e[n].content.charCodeAt(e[n].content.length - 1);
				break;
			}
			let h = 32;
			if (c < l) h = s.charCodeAt(c);
			else for (n = i + 1; n < e.length && e[n].type !== "softbreak" && e[n].type !== "hardbreak"; n++) if (e[n].content) {
				h = e[n].content.charCodeAt(0);
				break;
			}
			let g = Kt(m) || Gt(String.fromCharCode(m)), _ = Kt(h) || Gt(String.fromCharCode(h)), v = Wt(m), ee = Wt(h);
			if (ee ? d = !1 : _ && (v || g || (d = !1)), v ? f = !1 : g && (ee || _ || (f = !1)), h === 34 && u[0] === "\"" && m >= 48 && m <= 57 && (f = d = !1), d && f && (d = g, f = _), !d && !f) {
				p && (a.content = xn(a.content, u.index, bn));
				continue;
			}
			if (f) for (n = r.length - 1; n >= 0; n--) {
				let d = r[n];
				if (r[n].level < o) break;
				if (d.single === p && r[n].level === o) {
					d = r[n];
					let o, f;
					p ? (o = t.md.options.quotes[2], f = t.md.options.quotes[3]) : (o = t.md.options.quotes[0], f = t.md.options.quotes[1]), a.content = xn(a.content, u.index, f), e[d.token].content = xn(e[d.token].content, d.pos, o), c += f.length - 1, d.token === i && (c += o.length - 1), s = a.content, l = s.length, r.length = n;
					continue OUTER;
				}
			}
			d ? r.push({
				token: i,
				pos: u.index,
				single: p,
				level: o
			}) : f && p && (a.content = xn(a.content, u.index, bn));
		}
	}
}
function Cn(e) {
	if (e.md.options.typographer) for (let t = e.tokens.length - 1; t >= 0; t--) e.tokens[t].type === "inline" && vn.test(e.tokens[t].content) && Sn(e.tokens[t].children, e);
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_core/text_join.mjs
function wn(e) {
	let t, n, r = e.tokens, i = r.length;
	for (let e = 0; e < i; e++) {
		if (r[e].type !== "inline") continue;
		let i = r[e].children, a = i.length;
		for (t = 0; t < a; t++) i[t].type === "text_special" && (i[t].type = "text");
		for (t = n = 0; t < a; t++) i[t].type === "text" && t + 1 < a && i[t + 1].type === "text" ? i[t + 1].content = i[t].content + i[t + 1].content : (t !== n && (i[n] = i[t]), n++);
		t !== n && (i.length = n);
	}
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/parser_core.mjs
var Tn = [
	["normalize", rn],
	["block", an],
	["inline", on],
	["linkify", ln],
	["replacements", _n],
	["smartquotes", Cn],
	["text_join", wn]
];
function En() {
	this.ruler = new K();
	for (let e = 0; e < Tn.length; e++) this.ruler.push(Tn[e][0], Tn[e][1]);
}
En.prototype.process = function(e) {
	let t = this.ruler.getRules("");
	for (let n = 0, r = t.length; n < r; n++) t[n](e);
}, En.prototype.State = en;
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_block/state_block.mjs
function J(e, t, n, r) {
	this.src = e, this.md = t, this.env = n, this.tokens = r, this.bMarks = [], this.eMarks = [], this.tShift = [], this.sCount = [], this.bsCount = [], this.blkIndent = 0, this.line = 0, this.lineMax = 0, this.tight = !1, this.ddIndent = -1, this.listIndent = -1, this.parentType = "root", this.level = 0;
	let i = this.src;
	for (let e = 0, t = 0, n = 0, r = 0, a = i.length, o = !1; t < a; t++) {
		let s = i.charCodeAt(t);
		if (!o) {
			if (W(s)) {
				n++, s === 9 ? r += 4 - r % 4 : r++;
				continue;
			}
			o = !0;
		}
		(s === 10 || t === a - 1) && (s !== 10 && t++, this.bMarks.push(e), this.eMarks.push(t), this.tShift.push(n), this.sCount.push(r), this.bsCount.push(0), o = !1, n = 0, r = 0, e = t + 1);
	}
	this.bMarks.push(i.length), this.eMarks.push(i.length), this.tShift.push(0), this.sCount.push(0), this.bsCount.push(0), this.lineMax = this.bMarks.length - 1;
}
J.prototype.push = function(e, t, n) {
	let r = new q(e, t, n);
	return r.block = !0, n < 0 && this.level--, r.level = this.level, n > 0 && this.level++, this.tokens.push(r), r;
}, J.prototype.isEmpty = function(e) {
	return this.bMarks[e] + this.tShift[e] >= this.eMarks[e];
}, J.prototype.skipEmptyLines = function(e) {
	for (let t = this.lineMax; e < t && !(this.bMarks[e] + this.tShift[e] < this.eMarks[e]); e++);
	return e;
}, J.prototype.skipSpaces = function(e) {
	for (let t = this.src.length; e < t && W(this.src.charCodeAt(e)); e++);
	return e;
}, J.prototype.skipSpacesBack = function(e, t) {
	if (e <= t) return e;
	for (; e > t;) if (!W(this.src.charCodeAt(--e))) return e + 1;
	return e;
}, J.prototype.skipChars = function(e, t) {
	for (let n = this.src.length; e < n && this.src.charCodeAt(e) === t; e++);
	return e;
}, J.prototype.skipCharsBack = function(e, t, n) {
	if (e <= n) return e;
	for (; e > n;) if (t !== this.src.charCodeAt(--e)) return e + 1;
	return e;
}, J.prototype.getLines = function(e, t, n, r) {
	if (e >= t) return "";
	let i = Array(t - e);
	for (let a = 0, o = e; o < t; o++, a++) {
		let e = 0, s = this.bMarks[o], c = s, l;
		for (l = o + 1 < t || r ? this.eMarks[o] + 1 : this.eMarks[o]; c < l && e < n;) {
			let t = this.src.charCodeAt(c);
			if (W(t)) t === 9 ? e += 4 - (e + this.bsCount[o]) % 4 : e++;
			else if (c - s < this.tShift[o]) e++;
			else break;
			c++;
		}
		e > n ? i[a] = Array(e - n + 1).join(" ") + this.src.slice(c, l) : i[a] = this.src.slice(c, l);
	}
	return i.join("");
}, J.prototype.Token = q;
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_block/table.mjs
var Dn = 65536;
function On(e, t) {
	let n = e.bMarks[t] + e.tShift[t], r = e.eMarks[t];
	return e.src.slice(n, r);
}
function kn(e) {
	let t = [], n = e.length, r = 0, i = e.charCodeAt(r), a = !1, o = 0, s = "";
	for (; r < n;) i === 124 && (a ? (s += e.substring(o, r - 1), o = r) : (t.push(s + e.substring(o, r)), s = "", o = r + 1)), a = i === 92, r++, i = e.charCodeAt(r);
	return t.push(s + e.substring(o)), t;
}
function An(e, t, n, r) {
	if (t + 2 > n) return !1;
	let i = t + 1;
	if (e.sCount[i] < e.blkIndent || e.sCount[i] - e.blkIndent >= 4) return !1;
	let a = e.bMarks[i] + e.tShift[i];
	if (a >= e.eMarks[i]) return !1;
	let o = e.src.charCodeAt(a++);
	if (o !== 124 && o !== 45 && o !== 58 || a >= e.eMarks[i]) return !1;
	let s = e.src.charCodeAt(a++);
	if (s !== 124 && s !== 45 && s !== 58 && !W(s) || o === 45 && W(s)) return !1;
	for (; a < e.eMarks[i];) {
		let t = e.src.charCodeAt(a);
		if (t !== 124 && t !== 45 && t !== 58 && !W(t)) return !1;
		a++;
	}
	let c = On(e, t + 1), l = c.split("|"), u = [];
	for (let e = 0; e < l.length; e++) {
		let t = l[e].trim();
		if (!t) {
			if (e === 0 || e === l.length - 1) continue;
			return !1;
		}
		if (!/^:?-+:?$/.test(t)) return !1;
		t.charCodeAt(t.length - 1) === 58 ? u.push(t.charCodeAt(0) === 58 ? "center" : "right") : t.charCodeAt(0) === 58 ? u.push("left") : u.push("");
	}
	if (c = On(e, t).trim(), c.indexOf("|") === -1 || e.sCount[t] - e.blkIndent >= 4) return !1;
	l = kn(c), l.length && l[0] === "" && l.shift(), l.length && l[l.length - 1] === "" && l.pop();
	let d = l.length;
	if (d === 0 || d !== u.length) return !1;
	if (r) return !0;
	let f = e.parentType;
	e.parentType = "table";
	let p = e.md.block.ruler.getRules("blockquote"), m = e.push("table_open", "table", 1), h = [t, 0];
	m.map = h;
	let g = e.push("thead_open", "thead", 1);
	g.map = [t, t + 1];
	let _ = e.push("tr_open", "tr", 1);
	_.map = [t, t + 1];
	for (let t = 0; t < l.length; t++) {
		let n = e.push("th_open", "th", 1);
		u[t] && (n.attrs = [["style", "text-align:" + u[t]]]);
		let r = e.push("inline", "", 0);
		r.content = l[t].trim(), r.children = [], e.push("th_close", "th", -1);
	}
	e.push("tr_close", "tr", -1), e.push("thead_close", "thead", -1);
	let v, ee = 0;
	for (i = t + 2; i < n && !(e.sCount[i] < e.blkIndent); i++) {
		let r = !1;
		for (let t = 0, a = p.length; t < a; t++) if (p[t](e, i, n, !0)) {
			r = !0;
			break;
		}
		if (r || (c = On(e, i).trim(), !c) || e.sCount[i] - e.blkIndent >= 4 || (l = kn(c), l.length && l[0] === "" && l.shift(), l.length && l[l.length - 1] === "" && l.pop(), ee += d - l.length, ee > Dn)) break;
		if (i === t + 2) {
			let n = e.push("tbody_open", "tbody", 1);
			n.map = v = [t + 2, 0];
		}
		let a = e.push("tr_open", "tr", 1);
		a.map = [i, i + 1];
		for (let t = 0; t < d; t++) {
			let n = e.push("td_open", "td", 1);
			u[t] && (n.attrs = [["style", "text-align:" + u[t]]]);
			let r = e.push("inline", "", 0);
			r.content = l[t] ? l[t].trim() : "", r.children = [], e.push("td_close", "td", -1);
		}
		e.push("tr_close", "tr", -1);
	}
	return v && (e.push("tbody_close", "tbody", -1), v[1] = i), e.push("table_close", "table", -1), h[1] = i, e.parentType = f, e.line = i, !0;
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_block/code.mjs
function jn(e, t, n) {
	if (e.sCount[t] - e.blkIndent < 4) return !1;
	let r = t + 1, i = r;
	for (; r < n;) {
		if (e.isEmpty(r)) {
			r++;
			continue;
		}
		if (e.sCount[r] - e.blkIndent >= 4) {
			r++, i = r;
			continue;
		}
		break;
	}
	e.line = i;
	let a = e.push("code_block", "code", 0);
	return a.content = e.getLines(t, i, 4 + e.blkIndent, !1) + "\n", a.map = [t, e.line], !0;
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_block/fence.mjs
function Mn(e, t, n, r) {
	let i = e.bMarks[t] + e.tShift[t], a = e.eMarks[t];
	if (e.sCount[t] - e.blkIndent >= 4 || i + 3 > a) return !1;
	let o = e.src.charCodeAt(i);
	if (o !== 126 && o !== 96) return !1;
	let s = i;
	i = e.skipChars(i, o);
	let c = i - s;
	if (c < 3) return !1;
	let l = e.src.slice(s, i), u = e.src.slice(i, a);
	if (o === 96 && u.indexOf(String.fromCharCode(o)) >= 0) return !1;
	if (r) return !0;
	let d = t, f = !1;
	for (; d++, !(d >= n || (i = s = e.bMarks[d] + e.tShift[d], a = e.eMarks[d], i < a && e.sCount[d] < e.blkIndent));) if (e.src.charCodeAt(i) === o && !(e.sCount[d] - e.blkIndent >= 4) && (i = e.skipChars(i, o), !(i - s < c) && (i = e.skipSpaces(i), !(i < a)))) {
		f = !0;
		break;
	}
	c = e.sCount[t], e.line = d + +!!f;
	let p = e.push("fence", "code", 0);
	return p.info = u, p.content = e.getLines(t + 1, d, c, !0), p.markup = l, p.map = [t, e.line], !0;
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_block/blockquote.mjs
function Nn(e, t, n, r) {
	let i = e.bMarks[t] + e.tShift[t], a = e.eMarks[t], o = e.lineMax;
	if (e.sCount[t] - e.blkIndent >= 4 || e.src.charCodeAt(i) !== 62) return !1;
	if (r) return !0;
	let s = [], c = [], l = [], u = [], d = e.md.block.ruler.getRules("blockquote"), f = e.parentType;
	e.parentType = "blockquote";
	let p = !1, m;
	for (m = t; m < n; m++) {
		let t = e.sCount[m] < e.blkIndent;
		if (i = e.bMarks[m] + e.tShift[m], a = e.eMarks[m], i >= a) break;
		if (e.src.charCodeAt(i++) === 62 && !t) {
			let t = e.sCount[m] + 1, n, r;
			e.src.charCodeAt(i) === 32 ? (i++, t++, r = !1, n = !0) : e.src.charCodeAt(i) === 9 ? (n = !0, (e.bsCount[m] + t) % 4 == 3 ? (i++, t++, r = !1) : r = !0) : n = !1;
			let o = t;
			for (s.push(e.bMarks[m]), e.bMarks[m] = i; i < a;) {
				let t = e.src.charCodeAt(i);
				if (W(t)) t === 9 ? o += 4 - (o + e.bsCount[m] + +!!r) % 4 : o++;
				else break;
				i++;
			}
			p = i >= a, c.push(e.bsCount[m]), e.bsCount[m] = e.sCount[m] + 1 + +!!n, l.push(e.sCount[m]), e.sCount[m] = o - t, u.push(e.tShift[m]), e.tShift[m] = i - e.bMarks[m];
			continue;
		}
		if (p) break;
		let r = !1;
		for (let t = 0, i = d.length; t < i; t++) if (d[t](e, m, n, !0)) {
			r = !0;
			break;
		}
		if (r) {
			e.lineMax = m, e.blkIndent !== 0 && (s.push(e.bMarks[m]), c.push(e.bsCount[m]), u.push(e.tShift[m]), l.push(e.sCount[m]), e.sCount[m] -= e.blkIndent);
			break;
		}
		s.push(e.bMarks[m]), c.push(e.bsCount[m]), u.push(e.tShift[m]), l.push(e.sCount[m]), e.sCount[m] = -1;
	}
	let h = e.blkIndent;
	e.blkIndent = 0;
	let g = e.push("blockquote_open", "blockquote", 1);
	g.markup = ">";
	let _ = [t, 0];
	g.map = _, e.md.block.tokenize(e, t, m);
	let v = e.push("blockquote_close", "blockquote", -1);
	v.markup = ">", e.lineMax = o, e.parentType = f, _[1] = e.line;
	for (let n = 0; n < u.length; n++) e.bMarks[n + t] = s[n], e.tShift[n + t] = u[n], e.sCount[n + t] = l[n], e.bsCount[n + t] = c[n];
	return e.blkIndent = h, !0;
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_block/hr.mjs
function Pn(e, t, n, r) {
	let i = e.eMarks[t];
	if (e.sCount[t] - e.blkIndent >= 4) return !1;
	let a = e.bMarks[t] + e.tShift[t], o = e.src.charCodeAt(a++);
	if (o !== 42 && o !== 45 && o !== 95) return !1;
	let s = 1;
	for (; a < i;) {
		let t = e.src.charCodeAt(a++);
		if (t !== o && !W(t)) return !1;
		t === o && s++;
	}
	if (s < 3) return !1;
	if (r) return !0;
	e.line = t + 1;
	let c = e.push("hr", "hr", 0);
	return c.map = [t, e.line], c.markup = Array(s + 1).join(String.fromCharCode(o)), !0;
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_block/list.mjs
function Fn(e, t) {
	let n = e.eMarks[t], r = e.bMarks[t] + e.tShift[t], i = e.src.charCodeAt(r++);
	return i !== 42 && i !== 45 && i !== 43 || r < n && !W(e.src.charCodeAt(r)) ? -1 : r;
}
function In(e, t) {
	let n = e.bMarks[t] + e.tShift[t], r = e.eMarks[t], i = n;
	if (i + 1 >= r) return -1;
	let a = e.src.charCodeAt(i++);
	if (a < 48 || a > 57) return -1;
	for (;;) {
		if (i >= r) return -1;
		if (a = e.src.charCodeAt(i++), a >= 48 && a <= 57) {
			if (i - n >= 10) return -1;
			continue;
		}
		if (a === 41 || a === 46) break;
		return -1;
	}
	return i < r && (a = e.src.charCodeAt(i), !W(a)) ? -1 : i;
}
function Ln(e, t) {
	let n = e.level + 2;
	for (let r = t + 2, i = e.tokens.length - 2; r < i; r++) e.tokens[r].level === n && e.tokens[r].type === "paragraph_open" && (e.tokens[r + 2].hidden = !0, e.tokens[r].hidden = !0, r += 2);
}
function Rn(e, t, n, r) {
	let i, a, o, s, c = t, l = !0;
	if (e.sCount[c] - e.blkIndent >= 4 || e.listIndent >= 0 && e.sCount[c] - e.listIndent >= 4 && e.sCount[c] < e.blkIndent) return !1;
	let u = !1;
	r && e.parentType === "paragraph" && e.sCount[c] >= e.blkIndent && (u = !0);
	let d, f, p;
	if ((p = In(e, c)) >= 0) {
		if (d = !0, o = e.bMarks[c] + e.tShift[c], f = Number(e.src.slice(o, p - 1)), u && f !== 1) return !1;
	} else if ((p = Fn(e, c)) >= 0) d = !1;
	else return !1;
	if (u && e.skipSpaces(p) >= e.eMarks[c]) return !1;
	if (r) return !0;
	let m = e.src.charCodeAt(p - 1), h = e.tokens.length;
	d ? (s = e.push("ordered_list_open", "ol", 1), f !== 1 && (s.attrs = [["start", f]])) : s = e.push("bullet_list_open", "ul", 1);
	let g = [c, 0];
	s.map = g, s.markup = String.fromCharCode(m);
	let _ = !1, v = e.md.block.ruler.getRules("list"), ee = e.parentType;
	for (e.parentType = "list"; c < n;) {
		a = p, i = e.eMarks[c];
		let t = e.sCount[c] + p - (e.bMarks[c] + e.tShift[c]), r = t;
		for (; a < i;) {
			let t = e.src.charCodeAt(a);
			if (t === 9) r += 4 - (r + e.bsCount[c]) % 4;
			else if (t === 32) r++;
			else break;
			a++;
		}
		let u = a, f;
		f = u >= i ? 1 : r - t, f > 4 && (f = 1);
		let h = t + f;
		s = e.push("list_item_open", "li", 1), s.markup = String.fromCharCode(m);
		let g = [c, 0];
		s.map = g, d && (s.info = e.src.slice(o, p - 1));
		let ee = e.tight, te = e.tShift[c], y = e.sCount[c], b = e.listIndent;
		if (e.listIndent = e.blkIndent, e.blkIndent = h, e.tight = !0, e.tShift[c] = u - e.bMarks[c], e.sCount[c] = r, u >= i && e.isEmpty(c + 1) ? e.line = Math.min(e.line + 2, n) : e.md.block.tokenize(e, c, n, !0), (!e.tight || _) && (l = !1), _ = e.line - c > 1 && e.isEmpty(e.line - 1), e.blkIndent = e.listIndent, e.listIndent = b, e.tShift[c] = te, e.sCount[c] = y, e.tight = ee, s = e.push("list_item_close", "li", -1), s.markup = String.fromCharCode(m), c = e.line, g[1] = c, c >= n || e.sCount[c] < e.blkIndent || e.sCount[c] - e.blkIndent >= 4) break;
		let ne = !1;
		for (let t = 0, r = v.length; t < r; t++) if (v[t](e, c, n, !0)) {
			ne = !0;
			break;
		}
		if (ne) break;
		if (d) {
			if (p = In(e, c), p < 0) break;
			o = e.bMarks[c] + e.tShift[c];
		} else if (p = Fn(e, c), p < 0) break;
		if (m !== e.src.charCodeAt(p - 1)) break;
	}
	return s = d ? e.push("ordered_list_close", "ol", -1) : e.push("bullet_list_close", "ul", -1), s.markup = String.fromCharCode(m), g[1] = c, e.line = c, e.parentType = ee, l && Ln(e, h), !0;
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_block/reference.mjs
function zn(e, t, n, r) {
	let i = e.bMarks[t] + e.tShift[t], a = e.eMarks[t], o = t + 1;
	if (e.sCount[t] - e.blkIndent >= 4 || e.src.charCodeAt(i) !== 91) return !1;
	function s(t) {
		let n = e.lineMax;
		if (t >= n || e.isEmpty(t)) return null;
		let r = !1;
		if (e.sCount[t] - e.blkIndent > 3 && (r = !0), e.sCount[t] < 0 && (r = !0), !r) {
			let r = e.md.block.ruler.getRules("reference"), i = e.parentType;
			e.parentType = "reference";
			let a = !1;
			for (let i = 0, o = r.length; i < o; i++) if (r[i](e, t, n, !0)) {
				a = !0;
				break;
			}
			if (e.parentType = i, a) return null;
		}
		let i = e.bMarks[t] + e.tShift[t], a = e.eMarks[t];
		return e.src.slice(i, a + 1);
	}
	let c = e.src.slice(i, a + 1);
	a = c.length;
	let l = -1;
	for (i = 1; i < a; i++) {
		let e = c.charCodeAt(i);
		if (e === 91) return !1;
		if (e === 93) {
			l = i;
			break;
		}
		if (e === 10) {
			let e = s(o);
			e !== null && (c += e, a = c.length, o++);
		} else if (e === 92 && (i++, i < a && c.charCodeAt(i) === 10)) {
			let e = s(o);
			e !== null && (c += e, a = c.length, o++);
		}
	}
	if (l < 0 || c.charCodeAt(l + 1) !== 58) return !1;
	for (i = l + 2; i < a; i++) {
		let e = c.charCodeAt(i);
		if (e === 10) {
			let e = s(o);
			e !== null && (c += e, a = c.length, o++);
		} else if (!W(e)) break;
	}
	let u = e.md.helpers.parseLinkDestination(c, i, a);
	if (!u.ok) return !1;
	let d = e.md.normalizeLink(u.str);
	if (!e.md.validateLink(d)) return !1;
	i = u.pos;
	let f = i, p = o, m = i;
	for (; i < a; i++) {
		let e = c.charCodeAt(i);
		if (e === 10) {
			let e = s(o);
			e !== null && (c += e, a = c.length, o++);
		} else if (!W(e)) break;
	}
	let h = e.md.helpers.parseLinkTitle(c, i, a);
	for (; h.can_continue;) {
		let t = s(o);
		if (t === null) break;
		c += t, i = a, a = c.length, o++, h = e.md.helpers.parseLinkTitle(c, i, a, h);
	}
	let g;
	for (i < a && m !== i && h.ok ? (g = h.str, i = h.pos) : (g = "", i = f, o = p); i < a && W(c.charCodeAt(i));) i++;
	if (i < a && c.charCodeAt(i) !== 10 && g) for (g = "", i = f, o = p; i < a && W(c.charCodeAt(i));) i++;
	if (i < a && c.charCodeAt(i) !== 10) return !1;
	let _ = qt(c.slice(1, l));
	return _ ? 
	/* istanbul ignore if */
	r ? !0 : (e.env.references === void 0 && (e.env.references = {}), e.env.references[_] === void 0 && (e.env.references[_] = {
		title: g,
		href: d
	}), e.line = o, !0) : !1;
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/common/html_blocks.mjs
var Bn = /* @__PURE__ */ "address.article.aside.base.basefont.blockquote.body.caption.center.col.colgroup.dd.details.dialog.dir.div.dl.dt.fieldset.figcaption.figure.footer.form.frame.frameset.h1.h2.h3.h4.h5.h6.head.header.hr.html.iframe.legend.li.link.main.menu.menuitem.nav.noframes.ol.optgroup.option.p.param.search.section.summary.table.tbody.td.tfoot.th.thead.title.tr.track.ul".split("."), Vn = "<[A-Za-z][A-Za-z0-9\\-]*(?:\\s+[a-zA-Z_:][a-zA-Z0-9:._-]*(?:\\s*=\\s*(?:[^\"'=<>`\\x00-\\x20]+|'[^']*'|\"[^\"]*\"))?)*\\s*\\/?>", Hn = "<\\/[A-Za-z][A-Za-z0-9\\-]*\\s*>", Un = RegExp("^(?:" + Vn + "|" + Hn + "|<!---?>|<!--(?:[^-]|-[^-]|--[^>])*-->|<[?][\\s\\S]*?[?]>|<![A-Za-z][^>]*>|<!\\[CDATA\\[[\\s\\S]*?\\]\\]>)"), Wn = RegExp("^(?:" + Vn + "|" + Hn + ")"), Gn = [
	[
		/^<(script|pre|style|textarea)(?=(\s|>|$))/i,
		/<\/(script|pre|style|textarea)>/i,
		!0
	],
	[
		/^<!--/,
		/-->/,
		!0
	],
	[
		/^<\?/,
		/\?>/,
		!0
	],
	[
		/^<![A-Z]/,
		/>/,
		!0
	],
	[
		/^<!\[CDATA\[/,
		/\]\]>/,
		!0
	],
	[
		RegExp("^</?(" + Bn.join("|") + ")(?=(\\s|/?>|$))", "i"),
		/^$/,
		!0
	],
	[
		RegExp(Wn.source + "\\s*$"),
		/^$/,
		!1
	]
];
function Kn(e, t, n, r) {
	let i = e.bMarks[t] + e.tShift[t], a = e.eMarks[t];
	if (e.sCount[t] - e.blkIndent >= 4 || !e.md.options.html || e.src.charCodeAt(i) !== 60) return !1;
	let o = e.src.slice(i, a), s = 0;
	for (; s < Gn.length && !Gn[s][0].test(o); s++);
	if (s === Gn.length) return !1;
	if (r) return Gn[s][2];
	let c = t + 1;
	if (!Gn[s][1].test(o)) {
		for (; c < n && !(e.sCount[c] < e.blkIndent); c++) if (i = e.bMarks[c] + e.tShift[c], a = e.eMarks[c], o = e.src.slice(i, a), Gn[s][1].test(o)) {
			o.length !== 0 && c++;
			break;
		}
	}
	e.line = c;
	let l = e.push("html_block", "", 0);
	return l.map = [t, c], l.content = e.getLines(t, c, e.blkIndent, !0), !0;
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_block/heading.mjs
function qn(e, t, n, r) {
	let i = e.bMarks[t] + e.tShift[t], a = e.eMarks[t];
	if (e.sCount[t] - e.blkIndent >= 4) return !1;
	let o = e.src.charCodeAt(i);
	if (o !== 35 || i >= a) return !1;
	let s = 1;
	for (o = e.src.charCodeAt(++i); o === 35 && i < a && s <= 6;) s++, o = e.src.charCodeAt(++i);
	if (s > 6 || i < a && !W(o)) return !1;
	if (r) return !0;
	a = e.skipSpacesBack(a, i);
	let c = e.skipCharsBack(a, 35, i);
	c > i && W(e.src.charCodeAt(c - 1)) && (a = c), e.line = t + 1;
	let l = e.push("heading_open", "h" + String(s), 1);
	l.markup = "########".slice(0, s), l.map = [t, e.line];
	let u = e.push("inline", "", 0);
	u.content = e.src.slice(i, a).trim(), u.map = [t, e.line], u.children = [];
	let d = e.push("heading_close", "h" + String(s), -1);
	return d.markup = "########".slice(0, s), !0;
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_block/lheading.mjs
function Jn(e, t, n) {
	let r = e.md.block.ruler.getRules("paragraph");
	if (e.sCount[t] - e.blkIndent >= 4) return !1;
	let i = e.parentType;
	e.parentType = "paragraph";
	let a = 0, o, s = t + 1;
	for (; s < n && !e.isEmpty(s); s++) {
		if (e.sCount[s] - e.blkIndent > 3) continue;
		if (e.sCount[s] >= e.blkIndent) {
			let t = e.bMarks[s] + e.tShift[s], n = e.eMarks[s];
			if (t < n && (o = e.src.charCodeAt(t), (o === 45 || o === 61) && (t = e.skipChars(t, o), t = e.skipSpaces(t), t >= n))) {
				a = o === 61 ? 1 : 2;
				break;
			}
		}
		if (e.sCount[s] < 0) continue;
		let t = !1;
		for (let i = 0, a = r.length; i < a; i++) if (r[i](e, s, n, !0)) {
			t = !0;
			break;
		}
		if (t) break;
	}
	if (!a) return !1;
	let c = e.getLines(t, s, e.blkIndent, !1).trim();
	e.line = s + 1;
	let l = e.push("heading_open", "h" + String(a), 1);
	l.markup = String.fromCharCode(o), l.map = [t, e.line];
	let u = e.push("inline", "", 0);
	u.content = c, u.map = [t, e.line - 1], u.children = [];
	let d = e.push("heading_close", "h" + String(a), -1);
	return d.markup = String.fromCharCode(o), e.parentType = i, !0;
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_block/paragraph.mjs
function Yn(e, t, n) {
	let r = e.md.block.ruler.getRules("paragraph"), i = e.parentType, a = t + 1;
	for (e.parentType = "paragraph"; a < n && !e.isEmpty(a); a++) {
		if (e.sCount[a] - e.blkIndent > 3 || e.sCount[a] < 0) continue;
		let t = !1;
		for (let i = 0, o = r.length; i < o; i++) if (r[i](e, a, n, !0)) {
			t = !0;
			break;
		}
		if (t) break;
	}
	let o = e.getLines(t, a, e.blkIndent, !1).trim();
	e.line = a;
	let s = e.push("paragraph_open", "p", 1);
	s.map = [t, e.line];
	let c = e.push("inline", "", 0);
	return c.content = o, c.map = [t, e.line], c.children = [], e.push("paragraph_close", "p", -1), e.parentType = i, !0;
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/parser_block.mjs
var Xn = [
	[
		"table",
		An,
		["paragraph", "reference"]
	],
	["code", jn],
	[
		"fence",
		Mn,
		[
			"paragraph",
			"reference",
			"blockquote",
			"list"
		]
	],
	[
		"blockquote",
		Nn,
		[
			"paragraph",
			"reference",
			"blockquote",
			"list"
		]
	],
	[
		"hr",
		Pn,
		[
			"paragraph",
			"reference",
			"blockquote",
			"list"
		]
	],
	[
		"list",
		Rn,
		[
			"paragraph",
			"reference",
			"blockquote"
		]
	],
	["reference", zn],
	[
		"html_block",
		Kn,
		[
			"paragraph",
			"reference",
			"blockquote"
		]
	],
	[
		"heading",
		qn,
		[
			"paragraph",
			"reference",
			"blockquote"
		]
	],
	["lheading", Jn],
	["paragraph", Yn]
];
function Zn() {
	this.ruler = new K();
	for (let e = 0; e < Xn.length; e++) this.ruler.push(Xn[e][0], Xn[e][1], { alt: (Xn[e][2] || []).slice() });
}
Zn.prototype.tokenize = function(e, t, n) {
	let r = this.ruler.getRules(""), i = r.length, a = e.md.options.maxNesting, o = t, s = !1;
	for (; o < n && (e.line = o = e.skipEmptyLines(o), !(o >= n || e.sCount[o] < e.blkIndent));) {
		if (e.level >= a) {
			e.line = n;
			break;
		}
		let t = e.line, c = !1;
		for (let a = 0; a < i; a++) if (c = r[a](e, o, n, !1), c) {
			if (t >= e.line) throw Error("block rule didn't increment state.line");
			break;
		}
		if (!c) throw Error("none of the block rules matched");
		e.tight = !s, e.isEmpty(e.line - 1) && (s = !0), o = e.line, o < n && e.isEmpty(o) && (s = !0, o++, e.line = o);
	}
}, Zn.prototype.parse = function(e, t, n, r) {
	if (!e) return;
	let i = new this.State(e, t, n, r);
	this.tokenize(i, i.line, i.lineMax);
}, Zn.prototype.State = J;
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_inline/state_inline.mjs
function Qn(e, t, n, r) {
	this.src = e, this.env = n, this.md = t, this.tokens = r, this.tokens_meta = Array(r.length), this.pos = 0, this.posMax = this.src.length, this.level = 0, this.pending = "", this.pendingLevel = 0, this.cache = {}, this.delimiters = [], this._prev_delimiters = [], this.backticks = {}, this.backticksScanned = !1, this.linkLevel = 0;
}
Qn.prototype.pushPending = function() {
	let e = new q("text", "", 0);
	return e.content = this.pending, e.level = this.pendingLevel, this.tokens.push(e), this.pending = "", e;
}, Qn.prototype.push = function(e, t, n) {
	this.pending && this.pushPending();
	let r = new q(e, t, n), i = null;
	return n < 0 && (this.level--, this.delimiters = this._prev_delimiters.pop()), r.level = this.level, n > 0 && (this.level++, this._prev_delimiters.push(this.delimiters), this.delimiters = [], i = { delimiters: this.delimiters }), this.pendingLevel = this.level, this.tokens.push(r), this.tokens_meta.push(i), r;
}, Qn.prototype.scanDelims = function(e, t) {
	let n = this.posMax, r = this.src.charCodeAt(e), i = e > 0 ? this.src.charCodeAt(e - 1) : 32, a = e;
	for (; a < n && this.src.charCodeAt(a) === r;) a++;
	let o = a - e, s = a < n ? this.src.charCodeAt(a) : 32, c = Kt(i) || Gt(String.fromCharCode(i)), l = Kt(s) || Gt(String.fromCharCode(s)), u = Wt(i), d = Wt(s), f = !d && (!l || u || c), p = !u && (!c || d || l);
	return {
		can_open: f && (t || !p || c),
		can_close: p && (t || !f || l),
		length: o
	};
}, Qn.prototype.Token = q;
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_inline/text.mjs
function $n(e) {
	switch (e) {
		case 10:
		case 33:
		case 35:
		case 36:
		case 37:
		case 38:
		case 42:
		case 43:
		case 45:
		case 58:
		case 60:
		case 61:
		case 62:
		case 64:
		case 91:
		case 92:
		case 93:
		case 94:
		case 95:
		case 96:
		case 123:
		case 125:
		case 126: return !0;
		default: return !1;
	}
}
function er(e, t) {
	let n = e.pos;
	for (; n < e.posMax && !$n(e.src.charCodeAt(n));) n++;
	return n !== e.pos && (t || (e.pending += e.src.slice(e.pos, n)), e.pos = n, !0);
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_inline/linkify.mjs
var tr = /(?:^|[^a-z0-9.+-])([a-z][a-z0-9.+-]*)$/i;
function nr(e, t) {
	if (!e.md.options.linkify || e.linkLevel > 0) return !1;
	let n = e.pos, r = e.posMax;
	if (n + 3 > r || e.src.charCodeAt(n) !== 58 || e.src.charCodeAt(n + 1) !== 47 || e.src.charCodeAt(n + 2) !== 47) return !1;
	let i = e.pending.match(tr);
	if (!i) return !1;
	let a = i[1], o = e.md.linkify.matchAtStart(e.src.slice(n - a.length));
	if (!o) return !1;
	let s = o.url;
	if (s.length <= a.length) return !1;
	s = s.replace(/\*+$/, "");
	let c = e.md.normalizeLink(s);
	if (!e.md.validateLink(c)) return !1;
	if (!t) {
		e.pending = e.pending.slice(0, -a.length);
		let t = e.push("link_open", "a", 1);
		t.attrs = [["href", c]], t.markup = "linkify", t.info = "auto";
		let n = e.push("text", "", 0);
		n.content = e.md.normalizeLinkText(s);
		let r = e.push("link_close", "a", -1);
		r.markup = "linkify", r.info = "auto";
	}
	return e.pos += s.length - a.length, !0;
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_inline/newline.mjs
function rr(e, t) {
	let n = e.pos;
	if (e.src.charCodeAt(n) !== 10) return !1;
	let r = e.pending.length - 1, i = e.posMax;
	if (!t) {
		if (r >= 0 && e.pending.charCodeAt(r) === 32) {
			if (r >= 1 && e.pending.charCodeAt(r - 1) === 32) {
				let t = r - 1;
				for (; t >= 1 && e.pending.charCodeAt(t - 1) === 32;) t--;
				e.pending = e.pending.slice(0, t), e.push("hardbreak", "br", 0);
			} else e.pending = e.pending.slice(0, -1), e.push("softbreak", "br", 0);
		} else e.push("softbreak", "br", 0);
	}
	for (n++; n < i && W(e.src.charCodeAt(n));) n++;
	return e.pos = n, !0;
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_inline/escape.mjs
var ir = [];
for (let e = 0; e < 256; e++) ir.push(0);
"\\!\"#$%&'()*+,./:;<=>?@[]^_`{|}~-".split("").forEach(function(e) {
	ir[e.charCodeAt(0)] = 1;
});
function ar(e, t) {
	let n = e.pos, r = e.posMax;
	if (e.src.charCodeAt(n) !== 92 || (n++, n >= r)) return !1;
	let i = e.src.charCodeAt(n);
	if (i === 10) {
		for (t || e.push("hardbreak", "br", 0), n++; n < r && (i = e.src.charCodeAt(n), W(i));) n++;
		return e.pos = n, !0;
	}
	let a = e.src[n];
	if (i >= 55296 && i <= 56319 && n + 1 < r) {
		let t = e.src.charCodeAt(n + 1);
		t >= 56320 && t <= 57343 && (a += e.src[n + 1], n++);
	}
	let o = "\\" + a;
	if (!t) {
		let t = e.push("text_special", "", 0);
		t.content = i < 256 && ir[i] !== 0 ? a : o, t.markup = o, t.info = "escape";
	}
	return e.pos = n + 1, !0;
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_inline/backticks.mjs
function or(e, t) {
	let n = e.pos;
	if (e.src.charCodeAt(n) !== 96) return !1;
	let r = n;
	n++;
	let i = e.posMax;
	for (; n < i && e.src.charCodeAt(n) === 96;) n++;
	let a = e.src.slice(r, n), o = a.length;
	if (e.backticksScanned && (e.backticks[o] || 0) <= r) return t || (e.pending += a), e.pos += o, !0;
	let s = n, c;
	for (; (c = e.src.indexOf("`", s)) !== -1;) {
		for (s = c + 1; s < i && e.src.charCodeAt(s) === 96;) s++;
		let r = s - c;
		if (r === o) {
			if (!t) {
				let t = e.push("code_inline", "code", 0);
				t.markup = a, t.content = e.src.slice(n, c).replace(/\n/g, " ").replace(/^ (.+) $/, "$1");
			}
			return e.pos = s, !0;
		}
		e.backticks[r] = c;
	}
	return e.backticksScanned = !0, t || (e.pending += a), e.pos += o, !0;
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_inline/strikethrough.mjs
function sr(e, t) {
	let n = e.pos, r = e.src.charCodeAt(n);
	if (t || r !== 126) return !1;
	let i = e.scanDelims(e.pos, !0), a = i.length, o = String.fromCharCode(r);
	if (a < 2) return !1;
	let s;
	a % 2 && (s = e.push("text", "", 0), s.content = o, a--);
	for (let t = 0; t < a; t += 2) s = e.push("text", "", 0), s.content = o + o, e.delimiters.push({
		marker: r,
		length: 0,
		token: e.tokens.length - 1,
		end: -1,
		open: i.can_open,
		close: i.can_close
	});
	return e.pos += i.length, !0;
}
function cr(e, t) {
	let n, r = [], i = t.length;
	for (let a = 0; a < i; a++) {
		let i = t[a];
		if (i.marker !== 126 || i.end === -1) continue;
		let o = t[i.end];
		n = e.tokens[i.token], n.type = "s_open", n.tag = "s", n.nesting = 1, n.markup = "~~", n.content = "", n = e.tokens[o.token], n.type = "s_close", n.tag = "s", n.nesting = -1, n.markup = "~~", n.content = "", e.tokens[o.token - 1].type === "text" && e.tokens[o.token - 1].content === "~" && r.push(o.token - 1);
	}
	for (; r.length;) {
		let t = r.pop(), i = t + 1;
		for (; i < e.tokens.length && e.tokens[i].type === "s_close";) i++;
		i--, t !== i && (n = e.tokens[i], e.tokens[i] = e.tokens[t], e.tokens[t] = n);
	}
}
function lr(e) {
	let t = e.tokens_meta, n = e.tokens_meta.length;
	cr(e, e.delimiters);
	for (let r = 0; r < n; r++) t[r] && t[r].delimiters && cr(e, t[r].delimiters);
}
var ur = {
	tokenize: sr,
	postProcess: lr
};
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_inline/emphasis.mjs
function dr(e, t) {
	let n = e.pos, r = e.src.charCodeAt(n);
	if (t || r !== 95 && r !== 42) return !1;
	let i = e.scanDelims(e.pos, r === 42);
	for (let t = 0; t < i.length; t++) {
		let t = e.push("text", "", 0);
		t.content = String.fromCharCode(r), e.delimiters.push({
			marker: r,
			length: i.length,
			token: e.tokens.length - 1,
			end: -1,
			open: i.can_open,
			close: i.can_close
		});
	}
	return e.pos += i.length, !0;
}
function fr(e, t) {
	let n = t.length;
	for (let r = n - 1; r >= 0; r--) {
		let n = t[r];
		if (n.marker !== 95 && n.marker !== 42 || n.end === -1) continue;
		let i = t[n.end], a = r > 0 && t[r - 1].end === n.end + 1 && t[r - 1].marker === n.marker && t[r - 1].token === n.token - 1 && t[n.end + 1].token === i.token + 1, o = String.fromCharCode(n.marker), s = e.tokens[n.token];
		s.type = a ? "strong_open" : "em_open", s.tag = a ? "strong" : "em", s.nesting = 1, s.markup = a ? o + o : o, s.content = "";
		let c = e.tokens[i.token];
		c.type = a ? "strong_close" : "em_close", c.tag = a ? "strong" : "em", c.nesting = -1, c.markup = a ? o + o : o, c.content = "", a && (e.tokens[t[r - 1].token].content = "", e.tokens[t[n.end + 1].token].content = "", r--);
	}
}
function pr(e) {
	let t = e.tokens_meta, n = e.tokens_meta.length;
	fr(e, e.delimiters);
	for (let r = 0; r < n; r++) t[r] && t[r].delimiters && fr(e, t[r].delimiters);
}
var mr = {
	tokenize: dr,
	postProcess: pr
};
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_inline/link.mjs
function hr(e, t) {
	let n, r, i, a, o = "", s = "", c = e.pos, l = !0;
	if (e.src.charCodeAt(e.pos) !== 91) return !1;
	let u = e.pos, d = e.posMax, f = e.pos + 1, p = e.md.helpers.parseLinkLabel(e, e.pos, !0);
	if (p < 0) return !1;
	let m = p + 1;
	if (m < d && e.src.charCodeAt(m) === 40) {
		for (l = !1, m++; m < d && (n = e.src.charCodeAt(m), W(n) || n === 10); m++);
		if (m >= d) return !1;
		if (c = m, i = e.md.helpers.parseLinkDestination(e.src, m, e.posMax), i.ok) {
			for (o = e.md.normalizeLink(i.str), e.md.validateLink(o) ? m = i.pos : o = "", c = m; m < d && (n = e.src.charCodeAt(m), W(n) || n === 10); m++);
			if (i = e.md.helpers.parseLinkTitle(e.src, m, e.posMax), m < d && c !== m && i.ok) for (s = i.str, m = i.pos; m < d && (n = e.src.charCodeAt(m), W(n) || n === 10); m++);
		}
		(m >= d || e.src.charCodeAt(m) !== 41) && (l = !0), m++;
	}
	if (l) {
		if (e.env.references === void 0) return !1;
		if (m < d && e.src.charCodeAt(m) === 91 ? (c = m + 1, m = e.md.helpers.parseLinkLabel(e, m), m >= 0 ? r = e.src.slice(c, m++) : m = p + 1) : m = p + 1, r ||= e.src.slice(f, p), a = e.env.references[qt(r)], !a) return e.pos = u, !1;
		o = a.href, s = a.title;
	}
	if (!t) {
		e.pos = f, e.posMax = p;
		let t = e.push("link_open", "a", 1), n = [["href", o]];
		t.attrs = n, s && n.push(["title", s]), e.linkLevel++, e.md.inline.tokenize(e), e.linkLevel--, e.push("link_close", "a", -1);
	}
	return e.pos = m, e.posMax = d, !0;
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_inline/image.mjs
function gr(e, t) {
	let n, r, i, a, o, s, c, l, u = "", d = e.pos, f = e.posMax;
	if (e.src.charCodeAt(e.pos) !== 33 || e.src.charCodeAt(e.pos + 1) !== 91) return !1;
	let p = e.pos + 2, m = e.md.helpers.parseLinkLabel(e, e.pos + 1, !1);
	if (m < 0) return !1;
	if (a = m + 1, a < f && e.src.charCodeAt(a) === 40) {
		for (a++; a < f && (n = e.src.charCodeAt(a), W(n) || n === 10); a++);
		if (a >= f) return !1;
		for (l = a, s = e.md.helpers.parseLinkDestination(e.src, a, e.posMax), s.ok && (u = e.md.normalizeLink(s.str), e.md.validateLink(u) ? a = s.pos : u = ""), l = a; a < f && (n = e.src.charCodeAt(a), W(n) || n === 10); a++);
		if (s = e.md.helpers.parseLinkTitle(e.src, a, e.posMax), a < f && l !== a && s.ok) for (c = s.str, a = s.pos; a < f && (n = e.src.charCodeAt(a), W(n) || n === 10); a++);
		else c = "";
		if (a >= f || e.src.charCodeAt(a) !== 41) return e.pos = d, !1;
		a++;
	} else {
		if (e.env.references === void 0) return !1;
		if (a < f && e.src.charCodeAt(a) === 91 ? (l = a + 1, a = e.md.helpers.parseLinkLabel(e, a), a >= 0 ? i = e.src.slice(l, a++) : a = m + 1) : a = m + 1, i ||= e.src.slice(p, m), o = e.env.references[qt(i)], !o) return e.pos = d, !1;
		u = o.href, c = o.title;
	}
	if (!t) {
		r = e.src.slice(p, m);
		let t = [];
		e.md.inline.parse(r, e.md, e.env, t);
		let n = e.push("image", "img", 0), i = [["src", u], ["alt", ""]];
		n.attrs = i, n.children = t, n.content = r, c && i.push(["title", c]);
	}
	return e.pos = a, e.posMax = f, !0;
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_inline/autolink.mjs
var _r = /^([a-zA-Z0-9.!#$%&'*+/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*)$/, vr = /^([a-zA-Z][a-zA-Z0-9+.-]{1,31}):([^<>\x00-\x20]*)$/;
function yr(e, t) {
	let n = e.pos;
	if (e.src.charCodeAt(n) !== 60) return !1;
	let r = e.pos, i = e.posMax;
	for (;;) {
		if (++n >= i) return !1;
		let t = e.src.charCodeAt(n);
		if (t === 60) return !1;
		if (t === 62) break;
	}
	let a = e.src.slice(r + 1, n);
	if (vr.test(a)) {
		let n = e.md.normalizeLink(a);
		if (!e.md.validateLink(n)) return !1;
		if (!t) {
			let t = e.push("link_open", "a", 1);
			t.attrs = [["href", n]], t.markup = "autolink", t.info = "auto";
			let r = e.push("text", "", 0);
			r.content = e.md.normalizeLinkText(a);
			let i = e.push("link_close", "a", -1);
			i.markup = "autolink", i.info = "auto";
		}
		return e.pos += a.length + 2, !0;
	}
	if (_r.test(a)) {
		let n = e.md.normalizeLink("mailto:" + a);
		if (!e.md.validateLink(n)) return !1;
		if (!t) {
			let t = e.push("link_open", "a", 1);
			t.attrs = [["href", n]], t.markup = "autolink", t.info = "auto";
			let r = e.push("text", "", 0);
			r.content = e.md.normalizeLinkText(a);
			let i = e.push("link_close", "a", -1);
			i.markup = "autolink", i.info = "auto";
		}
		return e.pos += a.length + 2, !0;
	}
	return !1;
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_inline/html_inline.mjs
function br(e) {
	return /^<a[>\s]/i.test(e);
}
function xr(e) {
	return /^<\/a\s*>/i.test(e);
}
function Sr(e) {
	let t = e | 32;
	return t >= 97 && t <= 122;
}
function Cr(e, t) {
	if (!e.md.options.html) return !1;
	let n = e.posMax, r = e.pos;
	if (e.src.charCodeAt(r) !== 60 || r + 2 >= n) return !1;
	let i = e.src.charCodeAt(r + 1);
	if (i !== 33 && i !== 63 && i !== 47 && !Sr(i)) return !1;
	let a = e.src.slice(r).match(Un);
	if (!a) return !1;
	if (!t) {
		let t = e.push("html_inline", "", 0);
		t.content = a[0], br(t.content) && e.linkLevel++, xr(t.content) && e.linkLevel--;
	}
	return e.pos += a[0].length, !0;
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_inline/entity.mjs
var wr = /^&#((?:x[a-f0-9]{1,6}|[0-9]{1,7}));/i, Tr = /^&([a-z][a-z0-9]{1,31});/i;
function Er(e, t) {
	let n = e.pos, r = e.posMax;
	if (e.src.charCodeAt(n) !== 38 || n + 1 >= r) return !1;
	if (e.src.charCodeAt(n + 1) === 35) {
		let r = e.src.slice(n).match(wr);
		if (r) {
			if (!t) {
				let t = r[1][0].toLowerCase() === "x" ? parseInt(r[1].slice(1), 16) : parseInt(r[1], 10), n = e.push("text_special", "", 0);
				n.content = V(t) ? Mt(t) : Mt(65533), n.markup = r[0], n.info = "entity";
			}
			return e.pos += r[0].length, !0;
		}
	} else {
		let r = e.src.slice(n).match(Tr);
		if (r) {
			let n = z(r[0]);
			if (n !== r[0]) {
				if (!t) {
					let t = e.push("text_special", "", 0);
					t.content = n, t.markup = r[0], t.info = "entity";
				}
				return e.pos += r[0].length, !0;
			}
		}
	}
	return !1;
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_inline/balance_pairs.mjs
function Dr(e) {
	let t = {}, n = e.length;
	if (!n) return;
	let r = 0, i = -2, a = [];
	for (let o = 0; o < n; o++) {
		let n = e[o];
		if (a.push(0), (e[r].marker !== n.marker || i !== n.token - 1) && (r = o), i = n.token, n.length = n.length || 0, !n.close) continue;
		t.hasOwnProperty(n.marker) || (t[n.marker] = [
			-1,
			-1,
			-1,
			-1,
			-1,
			-1
		]);
		let s = t[n.marker][(n.open ? 3 : 0) + n.length % 3], c = r - a[r] - 1, l = c;
		for (; c > s; c -= a[c] + 1) {
			let t = e[c];
			if (t.marker === n.marker && t.open && t.end < 0) {
				let r = !1;
				if ((t.close || n.open) && (t.length + n.length) % 3 == 0 && (t.length % 3 != 0 || n.length % 3 != 0) && (r = !0), !r) {
					let r = c > 0 && !e[c - 1].open ? a[c - 1] + 1 : 0;
					a[o] = o - c + r, a[c] = r, n.open = !1, t.end = o, t.close = !1, l = -1, i = -2;
					break;
				}
			}
		}
		l !== -1 && (t[n.marker][(n.open ? 3 : 0) + (n.length || 0) % 3] = l);
	}
}
function Or(e) {
	let t = e.tokens_meta, n = e.tokens_meta.length;
	Dr(e.delimiters);
	for (let e = 0; e < n; e++) t[e] && t[e].delimiters && Dr(t[e].delimiters);
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/rules_inline/fragments_join.mjs
function kr(e) {
	let t, n, r = 0, i = e.tokens, a = e.tokens.length;
	for (t = n = 0; t < a; t++) i[t].nesting < 0 && r--, i[t].level = r, i[t].nesting > 0 && r++, i[t].type === "text" && t + 1 < a && i[t + 1].type === "text" ? i[t + 1].content = i[t].content + i[t + 1].content : (t !== n && (i[n] = i[t]), n++);
	t !== n && (i.length = n);
}
//#endregion
//#region ../../../node_modules/markdown-it/lib/parser_inline.mjs
var Ar = [
	["text", er],
	["linkify", nr],
	["newline", rr],
	["escape", ar],
	["backticks", or],
	["strikethrough", ur.tokenize],
	["emphasis", mr.tokenize],
	["link", hr],
	["image", gr],
	["autolink", yr],
	["html_inline", Cr],
	["entity", Er]
], jr = [
	["balance_pairs", Or],
	["strikethrough", ur.postProcess],
	["emphasis", mr.postProcess],
	["fragments_join", kr]
];
function Mr() {
	this.ruler = new K();
	for (let e = 0; e < Ar.length; e++) this.ruler.push(Ar[e][0], Ar[e][1]);
	this.ruler2 = new K();
	for (let e = 0; e < jr.length; e++) this.ruler2.push(jr[e][0], jr[e][1]);
}
Mr.prototype.skipToken = function(e) {
	let t = e.pos, n = this.ruler.getRules(""), r = n.length, i = e.md.options.maxNesting, a = e.cache;
	if (a[t] !== void 0) {
		e.pos = a[t];
		return;
	}
	let o = !1;
	if (e.level < i) {
		for (let i = 0; i < r; i++) if (e.level++, o = n[i](e, !0), e.level--, o) {
			if (t >= e.pos) throw Error("inline rule didn't increment state.pos");
			break;
		}
	} else e.pos = e.posMax;
	o || e.pos++, a[t] = e.pos;
}, Mr.prototype.tokenize = function(e) {
	let t = this.ruler.getRules(""), n = t.length, r = e.posMax, i = e.md.options.maxNesting;
	for (; e.pos < r;) {
		let a = e.pos, o = !1;
		if (e.level < i) {
			for (let r = 0; r < n; r++) if (o = t[r](e, !1), o) {
				if (a >= e.pos) throw Error("inline rule didn't increment state.pos");
				break;
			}
		}
		if (o) {
			if (e.pos >= r) break;
			continue;
		}
		e.pending += e.src[e.pos++];
	}
	e.pending && e.pushPending();
}, Mr.prototype.parse = function(e, t, n, r) {
	let i = new this.State(e, t, n, r);
	this.tokenize(i);
	let a = this.ruler2.getRules(""), o = a.length;
	for (let e = 0; e < o; e++) a[e](i);
}, Mr.prototype.State = Qn;
//#endregion
//#region ../../../node_modules/linkify-it/lib/re.mjs
function Nr(e) {
	let t = {};
	e ||= {}, t.src_Any = st.source, t.src_Cc = ct.source, t.src_Z = ft.source, t.src_P = ut.source, t.src_ZPCc = [
		t.src_Z,
		t.src_P,
		t.src_Cc
	].join("|"), t.src_ZCc = [t.src_Z, t.src_Cc].join("|");
	let n = "[><｜]";
	return t.src_pseudo_letter = `(?:(?!${n}|${t.src_ZPCc})${t.src_Any})`, t.src_ip4 = "(?:(25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\\.){3}(25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)", t.src_auth = `(?:(?:(?!${t.src_ZCc}|[@/\\[\\]()]).){1,50}@)?`, t.src_port = "(?::(?:6(?:[0-4]\\d{3}|5(?:[0-4]\\d{2}|5(?:[0-2]\\d|3[0-5])))|[1-5]?\\d{1,4}))?", t.src_host_terminator = `(?=$|${n}|${t.src_ZPCc})(?!${e["---"] ? "-(?!--)|" : "-|"}_|:\\d|\\.-|\\.(?!$|${t.src_ZPCc}))`, t.src_path = `(?:[/?#](?:(?!${t.src_ZCc}|${n}|[()[\\]{}.,"'?!\\-;]).|\\[(?:(?!${t.src_ZCc}|\\]).)*\\]|\\((?:(?!${t.src_ZCc}|[)]).)*\\)|\\{(?:(?!${t.src_ZCc}|[}]).)*\\}|\\"(?:(?!${t.src_ZCc}|["]).)+\\"|\\'(?:(?!${t.src_ZCc}|[']).)+\\'|\\'(?=${t.src_pseudo_letter}|[-])|\\.{2,}[a-zA-Z0-9%/&]|\\.(?!${t.src_ZCc}|[.]|$)|` + (e["---"] ? "\\-(?!--(?:[^-]|$))(?:-*)|" : "\\-+|") + `,(?!${t.src_ZCc}|$)|;(?!${t.src_ZCc}|$)|\\!+(?!${t.src_ZCc}|[!]|$)|\\?(?!${t.src_ZCc}|[?]|$))+|\\/)?`, t.src_email_name = "[\\-;:&=\\+\\$,\\.a-zA-Z0-9_][\\-;:&=\\+\\$,\\\"\\.a-zA-Z0-9_]{0,63}", t.src_xn = "xn--[a-z0-9\\-]{1,59}", t.src_domain_root = "(?:" + t.src_xn + `|${t.src_pseudo_letter}{1,63})`, t.src_domain = "(?:" + t.src_xn + `|(?:${t.src_pseudo_letter})|(?:${t.src_pseudo_letter}(?:-|${t.src_pseudo_letter}){0,61}${t.src_pseudo_letter}))`, t.src_host = `(?:(?:(?:(?:${t.src_domain})\\.)*${t.src_domain}))`, t.tpl_host_fuzzy = "(?:" + t.src_ip4 + `|(?:(?:(?:${t.src_domain})\\.)+(?:%TLDS%)))`, t.tpl_host_no_ip_fuzzy = `(?:(?:(?:${t.src_domain})\\.)+(?:%TLDS%))`, t.src_host_strict = t.src_host + t.src_host_terminator, t.tpl_host_fuzzy_strict = t.tpl_host_fuzzy + t.src_host_terminator, t.src_host_port_strict = t.src_host + t.src_port + t.src_host_terminator, t.tpl_host_port_fuzzy_strict = t.tpl_host_fuzzy + t.src_port + t.src_host_terminator, t.tpl_host_port_no_ip_fuzzy_strict = t.tpl_host_no_ip_fuzzy + t.src_port + t.src_host_terminator, t.tpl_host_fuzzy_test = `localhost|www\\.|\\.\\d{1,3}\\.|(?:\\.(?:%TLDS%)(?:${t.src_ZPCc}|>|$))`, t.tpl_email_fuzzy = `(^|${n}|"|\\(|${t.src_ZCc})(${t.src_email_name}@${t.tpl_host_fuzzy_strict})`, t.tpl_link_fuzzy = `(^|(?![.:/\\-_@])(?:[$+<=>^\`|\uff5c]|${t.src_ZPCc}))((?![$+<=>^\`|\uff5c])${t.tpl_host_port_fuzzy_strict}${t.src_path})`, t.tpl_link_no_ip_fuzzy = `(^|(?![.:/\\-_@])(?:[$+<=>^\`|\uff5c]|${t.src_ZPCc}))((?![$+<=>^\`|\uff5c])${t.tpl_host_port_no_ip_fuzzy_strict}${t.src_path})`, t;
}
//#endregion
//#region ../../../node_modules/linkify-it/index.mjs
function Pr(e) {
	return Array.prototype.slice.call(arguments, 1).forEach(function(t) {
		t && Object.keys(t).forEach(function(n) {
			e[n] = t[n];
		});
	}), e;
}
function Fr(e) {
	return Object.prototype.toString.call(e);
}
function Ir(e) {
	return Fr(e) === "[object String]";
}
function Lr(e) {
	return Fr(e) === "[object Object]";
}
function Rr(e) {
	return Fr(e) === "[object RegExp]";
}
function zr(e) {
	return Fr(e) === "[object Function]";
}
function Br(e) {
	return e.replace(/[.?*+^$[\]\\(){}|-]/g, "\\$&");
}
var Vr = {
	fuzzyLink: !0,
	fuzzyEmail: !0,
	fuzzyIP: !1
};
function Hr(e) {
	return Object.keys(e || {}).reduce(function(e, t) {
		return e || Vr.hasOwnProperty(t);
	}, !1);
}
var Ur = {
	"http:": { validate: function(e, t, n) {
		let r = e.slice(t);
		return n.re.http || (n.re.http = RegExp(`^\\/\\/${n.re.src_auth}${n.re.src_host_port_strict}${n.re.src_path}`, "i")), n.re.http.test(r) ? r.match(n.re.http)[0].length : 0;
	} },
	"https:": "http:",
	"ftp:": "http:",
	"//": { validate: function(e, t, n) {
		let r = e.slice(t);
		return n.re.no_http || (n.re.no_http = RegExp("^" + n.re.src_auth + `(?:localhost|(?:(?:${n.re.src_domain})\\.)+${n.re.src_domain_root})` + n.re.src_port + n.re.src_host_terminator + n.re.src_path, "i")), n.re.no_http.test(r) ? t >= 3 && e[t - 3] === ":" || t >= 3 && e[t - 3] === "/" ? 0 : r.match(n.re.no_http)[0].length : 0;
	} },
	"mailto:": { validate: function(e, t, n) {
		let r = e.slice(t);
		return n.re.mailto || (n.re.mailto = RegExp(`^${n.re.src_email_name}@${n.re.src_host_strict}`, "i")), n.re.mailto.test(r) ? r.match(n.re.mailto)[0].length : 0;
	} }
}, Wr = "a[cdefgilmnoqrstuwxz]|b[abdefghijmnorstvwyz]|c[acdfghiklmnoruvwxyz]|d[ejkmoz]|e[cegrstu]|f[ijkmor]|g[abdefghilmnpqrstuwy]|h[kmnrtu]|i[delmnoqrst]|j[emop]|k[eghimnprwyz]|l[abcikrstuvy]|m[acdeghklmnopqrstuvwxyz]|n[acefgilopruz]|om|p[aefghklmnrstwy]|qa|r[eosuw]|s[abcdeghijklmnortuvxyz]|t[cdfghjklmnortvwz]|u[agksyz]|v[aceginu]|w[fs]|y[et]|z[amw]", Gr = "biz|com|edu|gov|net|org|pro|web|xxx|aero|asia|coop|info|museum|name|shop|рф".split("|");
function Kr(e) {
	return function(t, n) {
		let r = t.slice(n);
		return e.test(r) ? r.match(e)[0].length : 0;
	};
}
function qr() {
	return function(e, t) {
		t.normalize(e);
	};
}
function Jr(e) {
	let t = e.re = Nr(e.__opts__), n = e.__tlds__.slice();
	e.onCompile(), e.__tlds_replaced__ || n.push(Wr), n.push(t.src_xn), t.src_tlds = n.join("|");
	function r(e) {
		return e.replace("%TLDS%", t.src_tlds);
	}
	t.email_fuzzy = RegExp(r(t.tpl_email_fuzzy), "i"), t.email_fuzzy_global = RegExp(r(t.tpl_email_fuzzy), "ig"), t.link_fuzzy = RegExp(r(t.tpl_link_fuzzy), "i"), t.link_fuzzy_global = RegExp(r(t.tpl_link_fuzzy), "ig"), t.link_no_ip_fuzzy = RegExp(r(t.tpl_link_no_ip_fuzzy), "i"), t.link_no_ip_fuzzy_global = RegExp(r(t.tpl_link_no_ip_fuzzy), "ig"), t.host_fuzzy_test = RegExp(r(t.tpl_host_fuzzy_test), "i");
	let i = [];
	e.__compiled__ = {};
	function a(e, t) {
		throw Error(`(LinkifyIt) Invalid schema "${e}": ${t}`);
	}
	Object.keys(e.__schemas__).forEach(function(t) {
		let n = e.__schemas__[t];
		if (n === null) return;
		let r = {
			validate: null,
			link: null
		};
		if (e.__compiled__[t] = r, Lr(n)) {
			Rr(n.validate) ? r.validate = Kr(n.validate) : zr(n.validate) ? r.validate = n.validate : a(t, n), zr(n.normalize) ? r.normalize = n.normalize : n.normalize ? a(t, n) : r.normalize = qr();
			return;
		}
		if (Ir(n)) {
			i.push(t);
			return;
		}
		a(t, n);
	}), i.forEach(function(t) {
		e.__compiled__[e.__schemas__[t]] && (e.__compiled__[t].validate = e.__compiled__[e.__schemas__[t]].validate, e.__compiled__[t].normalize = e.__compiled__[e.__schemas__[t]].normalize);
	}), e.__compiled__[""] = {
		validate: null,
		normalize: qr()
	};
	let o = Object.keys(e.__compiled__).filter(function(t) {
		return t.length > 0 && e.__compiled__[t];
	}).map(Br).join("|");
	e.re.schema_test = RegExp(`(^|(?!_)(?:[><\uff5c]|${t.src_ZPCc}))(${o})`, "i"), e.re.schema_search = RegExp(`(^|(?!_)(?:[><\uff5c]|${t.src_ZPCc}))(${o})`, "ig"), e.re.schema_at_start = RegExp(`^${e.re.schema_search.source}`, "i"), e.re.pretest = RegExp(`(${e.re.schema_test.source})|(${e.re.host_fuzzy_test.source})|@`, "i");
}
function Yr(e, t, n, r) {
	let i = e.slice(n, r);
	this.schema = t.toLowerCase(), this.index = n, this.lastIndex = r, this.raw = i, this.text = i, this.url = i;
}
function Y(e, t) {
	if (!(this instanceof Y)) return new Y(e, t);
	t || Hr(e) && (t = e, e = {}), this.__opts__ = Pr({}, Vr, t), this.__schemas__ = Pr({}, Ur, e), this.__compiled__ = {}, this.__tlds__ = Gr, this.__tlds_replaced__ = !1, this.re = {}, Jr(this);
}
Y.prototype.add = function(e, t) {
	return this.__schemas__[e] = t, Jr(this), this;
}, Y.prototype.set = function(e) {
	return this.__opts__ = Pr(this.__opts__, e), this;
}, Y.prototype.test = function(e) {
	if (!e.length) return !1;
	let t, n;
	if (this.re.schema_test.test(e)) {
		for (n = this.re.schema_search, n.lastIndex = 0; (t = n.exec(e)) !== null;) if (this.testSchemaAt(e, t[2], n.lastIndex)) return !0;
	}
	return !!(this.__opts__.fuzzyLink && this.__compiled__["http:"] && e.search(this.re.host_fuzzy_test) >= 0 && e.match(this.__opts__.fuzzyIP ? this.re.link_fuzzy : this.re.link_no_ip_fuzzy) !== null || this.__opts__.fuzzyEmail && this.__compiled__["mailto:"] && e.indexOf("@") >= 0 && e.match(this.re.email_fuzzy) !== null);
}, Y.prototype.pretest = function(e) {
	return this.re.pretest.test(e);
}, Y.prototype.testSchemaAt = function(e, t, n) {
	return this.__compiled__[t.toLowerCase()] ? this.__compiled__[t.toLowerCase()].validate(e, n, this) : 0;
}, Y.prototype.match = function(e) {
	let t = [], n = [], r = [], i = [], a, o, s;
	function c(e, t) {
		return e ? t ? e.index === t.index ? e.lastIndex >= t.lastIndex ? e : t : e.index < t.index ? e : t : e : t;
	}
	if (!e.length) return null;
	if (this.re.schema_test.test(e)) for (s = this.re.schema_search, s.lastIndex = 0; (a = s.exec(e)) !== null;) o = this.testSchemaAt(e, a[2], s.lastIndex), o && n.push({
		schema: a[2],
		index: a.index + a[1].length,
		lastIndex: a.index + a[0].length + o
	});
	if (this.__opts__.fuzzyLink && this.__compiled__["http:"]) for (s = this.__opts__.fuzzyIP ? this.re.link_fuzzy_global : this.re.link_no_ip_fuzzy_global, s.lastIndex = 0; (a = s.exec(e)) !== null;) r.push({
		schema: "",
		index: a.index + a[1].length,
		lastIndex: a.index + a[0].length
	});
	if (this.__opts__.fuzzyEmail && this.__compiled__["mailto:"]) for (s = this.re.email_fuzzy_global, s.lastIndex = 0; (a = s.exec(e)) !== null;) i.push({
		schema: "mailto:",
		index: a.index + a[1].length,
		lastIndex: a.index + a[0].length
	});
	let l = [
		0,
		0,
		0
	], u = 0;
	for (;;) {
		let a = [
			n[l[0]],
			i[l[1]],
			r[l[2]]
		], o = c(c(a[0], a[1]), a[2]);
		if (!o) break;
		if (o === a[0] ? l[0]++ : o === a[1] ? l[1]++ : l[2]++, o.index < u) continue;
		let s = new Yr(e, o.schema, o.index, o.lastIndex);
		this.__compiled__[s.schema].normalize(s, this), t.push(s), u = o.lastIndex;
	}
	return t.length ? t : null;
}, Y.prototype.matchAtStart = function(e) {
	if (!e.length) return null;
	let t = this.re.schema_at_start.exec(e);
	if (!t) return null;
	let n = this.testSchemaAt(e, t[2], t[0].length);
	if (!n) return null;
	let r = new Yr(e, t[2], t.index + t[1].length, t.index + t[0].length + n);
	return this.__compiled__[r.schema].normalize(r, this), r;
}, Y.prototype.tlds = function(e, t) {
	return e = Array.isArray(e) ? e : [e], t ? (this.__tlds__ = this.__tlds__.concat(e).sort().filter(function(e, t, n) {
		return e !== n[t - 1];
	}).reverse(), Jr(this), this) : (this.__tlds__ = e.slice(), this.__tlds_replaced__ = !0, Jr(this), this);
}, Y.prototype.normalize = function(e) {
	e.schema || (e.url = `http://${e.url}`), e.schema === "mailto:" && !/^mailto:/i.test(e.url) && (e.url = `mailto:${e.url}`);
}, Y.prototype.onCompile = function() {};
//#endregion
//#region ../../../node_modules/punycode.js/punycode.es6.js
var Xr = 2147483647, X = 36, Zr = 1, Qr = 26, $r = 38, ei = 700, ti = 72, ni = 128, ri = "-", ii = /^xn--/, ai = /[^\0-\x7F]/, oi = /[\x2E\u3002\uFF0E\uFF61]/g, si = {
	overflow: "Overflow: input needs wider integers to process",
	"not-basic": "Illegal input >= 0x80 (not a basic code point)",
	"invalid-input": "Invalid input"
}, ci = 35, Z = Math.floor, li = String.fromCharCode;
function ui(e) {
	throw RangeError(si[e]);
}
function di(e, t) {
	let n = [], r = e.length;
	for (; r--;) n[r] = t(e[r]);
	return n;
}
function fi(e, t) {
	let n = e.split("@"), r = "";
	n.length > 1 && (r = n[0] + "@", e = n[1]), e = e.replace(oi, ".");
	let i = di(e.split("."), t).join(".");
	return r + i;
}
function pi(e) {
	let t = [], n = 0, r = e.length;
	for (; n < r;) {
		let i = e.charCodeAt(n++);
		if (i >= 55296 && i <= 56319 && n < r) {
			let r = e.charCodeAt(n++);
			(r & 64512) == 56320 ? t.push(((i & 1023) << 10) + (r & 1023) + 65536) : (t.push(i), n--);
		} else t.push(i);
	}
	return t;
}
var mi = (e) => String.fromCodePoint(...e), hi = function(e) {
	return e >= 48 && e < 58 ? 26 + (e - 48) : e >= 65 && e < 91 ? e - 65 : e >= 97 && e < 123 ? e - 97 : X;
}, gi = function(e, t) {
	return e + 22 + 75 * (e < 26) - ((t != 0) << 5);
}, _i = function(e, t, n) {
	let r = 0;
	for (e = n ? Z(e / ei) : e >> 1, e += Z(e / t); e > 455; r += X) e = Z(e / ci);
	return Z(r + 36 * e / (e + $r));
}, vi = function(e) {
	let t = [], n = e.length, r = 0, i = ni, a = ti, o = e.lastIndexOf(ri);
	o < 0 && (o = 0);
	for (let n = 0; n < o; ++n) e.charCodeAt(n) >= 128 && ui("not-basic"), t.push(e.charCodeAt(n));
	for (let s = o > 0 ? o + 1 : 0; s < n;) {
		let o = r;
		for (let t = 1, i = X;; i += X) {
			s >= n && ui("invalid-input");
			let o = hi(e.charCodeAt(s++));
			o >= X && ui("invalid-input"), o > Z((Xr - r) / t) && ui("overflow"), r += o * t;
			let c = i <= a ? Zr : i >= a + Qr ? Qr : i - a;
			if (o < c) break;
			let l = X - c;
			t > Z(Xr / l) && ui("overflow"), t *= l;
		}
		let c = t.length + 1;
		a = _i(r - o, c, o == 0), Z(r / c) > Xr - i && ui("overflow"), i += Z(r / c), r %= c, t.splice(r++, 0, i);
	}
	return String.fromCodePoint(...t);
}, yi = function(e) {
	let t = [];
	e = pi(e);
	let n = e.length, r = ni, i = 0, a = ti;
	for (let n of e) n < 128 && t.push(li(n));
	let o = t.length, s = o;
	for (o && t.push(ri); s < n;) {
		let n = Xr;
		for (let t of e) t >= r && t < n && (n = t);
		let c = s + 1;
		n - r > Z((Xr - i) / c) && ui("overflow"), i += (n - r) * c, r = n;
		for (let n of e) if (n < r && ++i > Xr && ui("overflow"), n === r) {
			let e = i;
			for (let n = X;; n += X) {
				let r = n <= a ? Zr : n >= a + Qr ? Qr : n - a;
				if (e < r) break;
				let i = e - r, o = X - r;
				t.push(li(gi(r + i % o, 0))), e = Z(i / o);
			}
			t.push(li(gi(e, 0))), a = _i(i, c, s === o), i = 0, ++s;
		}
		++i, ++r;
	}
	return t.join("");
}, bi = {
	version: "2.3.1",
	ucs2: {
		decode: pi,
		encode: mi
	},
	decode: vi,
	encode: yi,
	toASCII: function(e) {
		return fi(e, function(e) {
			return ai.test(e) ? "xn--" + yi(e) : e;
		});
	},
	toUnicode: function(e) {
		return fi(e, function(e) {
			return ii.test(e) ? vi(e.slice(4).toLowerCase()) : e;
		});
	}
}, xi = {
	default: {
		options: {
			html: !1,
			xhtmlOut: !1,
			breaks: !1,
			langPrefix: "language-",
			linkify: !1,
			typographer: !1,
			quotes: "“”‘’",
			highlight: null,
			maxNesting: 100
		},
		components: {
			core: {},
			block: {},
			inline: {}
		}
	},
	zero: {
		options: {
			html: !1,
			xhtmlOut: !1,
			breaks: !1,
			langPrefix: "language-",
			linkify: !1,
			typographer: !1,
			quotes: "“”‘’",
			highlight: null,
			maxNesting: 20
		},
		components: {
			core: { rules: [
				"normalize",
				"block",
				"inline",
				"text_join"
			] },
			block: { rules: ["paragraph"] },
			inline: {
				rules: ["text"],
				rules2: ["balance_pairs", "fragments_join"]
			}
		}
	},
	commonmark: {
		options: {
			html: !0,
			xhtmlOut: !0,
			breaks: !1,
			langPrefix: "language-",
			linkify: !1,
			typographer: !1,
			quotes: "“”‘’",
			highlight: null,
			maxNesting: 20
		},
		components: {
			core: { rules: [
				"normalize",
				"block",
				"inline",
				"text_join"
			] },
			block: { rules: [
				"blockquote",
				"code",
				"fence",
				"heading",
				"hr",
				"html_block",
				"lheading",
				"list",
				"reference",
				"paragraph"
			] },
			inline: {
				rules: [
					"autolink",
					"backticks",
					"emphasis",
					"entity",
					"escape",
					"html_inline",
					"image",
					"link",
					"newline",
					"text"
				],
				rules2: [
					"balance_pairs",
					"emphasis",
					"fragments_join"
				]
			}
		}
	}
}, Si = /^(vbscript|javascript|file|data):/, Ci = /^data:image\/(gif|png|jpeg|webp);/;
function wi(e) {
	let t = e.trim().toLowerCase();
	return !Si.test(t) || Ci.test(t);
}
var Ti = [
	"http:",
	"https:",
	"mailto:"
];
function Ei(e) {
	let t = M(e, !0);
	if (t.hostname && (!t.protocol || Ti.indexOf(t.protocol) >= 0)) try {
		t.hostname = bi.toASCII(t.hostname);
	} catch {}
	return Xe(k(t));
}
function Di(e) {
	let t = M(e, !0);
	if (t.hostname && (!t.protocol || Ti.indexOf(t.protocol) >= 0)) try {
		t.hostname = bi.toUnicode(t.hostname);
	} catch {}
	return D(k(t), D.defaultChars + "%");
}
function Q(e, t) {
	if (!(this instanceof Q)) return new Q(e, t);
	t || Ot(e) || (t = e || {}, e = "default"), this.inline = new Mr(), this.block = new Zn(), this.core = new En(), this.renderer = new $t(), this.linkify = new Y(), this.validateLink = wi, this.normalizeLink = Ei, this.normalizeLinkText = Di, this.utils = Et, this.helpers = jt({}, Qt), this.options = {}, this.configure(e), t && this.set(t);
}
Q.prototype.set = function(e) {
	return jt(this.options, e), this;
}, Q.prototype.configure = function(e) {
	let t = this;
	if (Ot(e)) {
		let t = e;
		if (e = xi[t], !e) throw Error("Wrong `markdown-it` preset \"" + t + "\", check name");
	}
	if (!e) throw Error("Wrong `markdown-it` preset, can't be empty");
	return e.options && t.set(e.options), e.components && Object.keys(e.components).forEach(function(n) {
		e.components[n].rules && t[n].ruler.enableOnly(e.components[n].rules), e.components[n].rules2 && t[n].ruler2.enableOnly(e.components[n].rules2);
	}), this;
}, Q.prototype.enable = function(e, t) {
	let n = [];
	Array.isArray(e) || (e = [e]), [
		"core",
		"block",
		"inline"
	].forEach(function(t) {
		n = n.concat(this[t].ruler.enable(e, !0));
	}, this), n = n.concat(this.inline.ruler2.enable(e, !0));
	let r = e.filter(function(e) {
		return n.indexOf(e) < 0;
	});
	if (r.length && !t) throw Error("MarkdownIt. Failed to enable unknown rule(s): " + r);
	return this;
}, Q.prototype.disable = function(e, t) {
	let n = [];
	Array.isArray(e) || (e = [e]), [
		"core",
		"block",
		"inline"
	].forEach(function(t) {
		n = n.concat(this[t].ruler.disable(e, !0));
	}, this), n = n.concat(this.inline.ruler2.disable(e, !0));
	let r = e.filter(function(e) {
		return n.indexOf(e) < 0;
	});
	if (r.length && !t) throw Error("MarkdownIt. Failed to disable unknown rule(s): " + r);
	return this;
}, Q.prototype.use = function(e) {
	let t = [this].concat(Array.prototype.slice.call(arguments, 1));
	return e.apply(e, t), this;
}, Q.prototype.parse = function(e, t) {
	if (typeof e != "string") throw Error("Input data should be a String");
	let n = new this.core.State(e, this, t);
	return this.core.process(n), n.tokens;
}, Q.prototype.render = function(e, t) {
	return t ||= {}, this.renderer.render(this.parse(e, t), this.options, t);
}, Q.prototype.parseInline = function(e, t) {
	let n = new this.core.State(e, this, t);
	return n.inlineMode = !0, this.core.process(n), n.tokens;
}, Q.prototype.renderInline = function(e, t) {
	return t ||= {}, this.renderer.render(this.parseInline(e, t), this.options, t);
};
//#endregion
//#region src/markdown.ts
var Oi = /^(?:https?:|mailto:|tel:|#|\/|\.\.?\/)/i, ki = /^(?:(?:https?|mailto|tel):|[^a-z][^:]*$|#|\/|\.\.?\/)/i, Ai = [
	"a",
	"blockquote",
	"br",
	"code",
	"del",
	"em",
	"h1",
	"h2",
	"h3",
	"h4",
	"hr",
	"li",
	"ol",
	"p",
	"pre",
	"strong",
	"ul"
], ji = [
	"href",
	"target",
	"rel"
];
function Mi(e) {
	try {
		return Oi.test(decodeURIComponent(e).trim()) && ki.test(e.trim());
	} catch {
		return !1;
	}
}
function Ni(e) {
	return e.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll("\"", "&quot;");
}
var Pi = new Q({
	html: !1,
	breaks: !0,
	linkify: !1,
	typographer: !1
});
Pi.validateLink = Mi, Pi.renderer.rules.image = (e, t) => {
	let n = e[t];
	return Ni(n?.children?.map((e) => e.content).join("") || n?.content || n?.attrGet("alt") || "");
};
var Fi = Pi.renderer.rules.link_open;
Pi.renderer.rules.link_open = (e, t, n, r, i) => {
	let a = e[t];
	return a ? (Mi(a.attrGet("href") ?? "#") || a.attrSet("href", "#"), a.attrSet("target", "_blank"), a.attrSet("rel", "noopener noreferrer"), Fi ? Fi(e, t, n, r, i) : i.renderToken(e, t, n)) : "";
};
function Ii(e) {
	let t = e.replace(/\]\(([^)]*)\)/g, (e, t) => Mi(t.trim().split(/\s+/, 1)[0] ?? "") ? e : "]()"), n = Pi.render(t);
	return typeof window > "u" ? n : Ke.sanitize(n, {
		ALLOWED_TAGS: Ai,
		ALLOWED_ATTR: ji,
		FORBID_TAGS: [
			"img",
			"audio",
			"video",
			"iframe",
			"object",
			"embed",
			"style",
			"script"
		],
		FORBID_ATTR: [
			"style",
			"src",
			"srcset",
			"onerror",
			"onclick"
		],
		ALLOW_DATA_ATTR: !1,
		ALLOWED_URI_REGEXP: ki
	});
}
function Li(e) {
	return e.length <= 16 ? e : `${e.slice(0, 8)}…${e.slice(-6)}`;
}
function Ri(e, t, n) {
	return n || t !== void 0 && e === t ? "You" : Li(e);
}
//#endregion
//#region src/view.ts
var $ = {
	panel: "teamarness-panel teamarness-stack",
	control: "teamarness-control",
	primary: "teamarness-control teamarness-control--primary",
	secondary: "teamarness-control teamarness-control--secondary",
	field: "teamarness-field",
	input: "teamarness-input",
	notice: "teamarness-notice",
	noticeDanger: "teamarness-notice teamarness-notice--danger"
}, zi = class {
	root;
	controller;
	shell;
	rail;
	panel;
	headerSlot;
	history;
	olderButton;
	composer;
	textarea;
	sendButton;
	disposed = !1;
	constructor(e, t) {
		this.root = e, this.controller = t, this.shell = document.createElement("div"), this.shell.className = "conversations-shell", this.shell.dataset.narrowLayout = "true", this.rail = document.createElement("aside"), this.rail.className = "channel-rail", this.rail.setAttribute("aria-label", "Channels"), this.panel = document.createElement("section"), this.panel.className = `conversation-panel ${$.panel}`, this.headerSlot = document.createElement("div"), this.panel.append(this.headerSlot), this.history = document.createElement("ol"), this.history.className = "message-history", this.history.setAttribute("aria-label", "Messages"), this.panel.append(this.history), this.olderButton = document.createElement("button"), this.olderButton.type = "button", this.olderButton.className = `${$.secondary} older-messages`, this.olderButton.addEventListener("click", () => {
			t.loadOlder();
		}), this.panel.append(this.olderButton), this.composer = document.createElement("form"), this.composer.className = `composer ${$.field}`;
		let n = document.createElement("label");
		n.htmlFor = "message-markdown", n.textContent = "Message (Markdown)", this.composer.append(n), this.textarea = document.createElement("textarea"), this.textarea.id = "message-markdown", this.textarea.name = "markdown", this.textarea.className = $.input, this.textarea.required = !0, this.textarea.maxLength = 32768, this.textarea.placeholder = "Write a message…", this.textarea.addEventListener("input", () => {
			t.draft = this.textarea.value;
		}), this.composer.append(this.textarea), this.sendButton = document.createElement("button"), this.sendButton.type = "submit", this.sendButton.className = $.primary, this.sendButton.textContent = "Send", this.composer.append(this.sendButton), this.composer.addEventListener("submit", (e) => {
			e.preventDefault();
			let n = this.textarea.value;
			t.postMessage(n).then(() => {
				this.disposed || !t.state.loading && !t.state.warning && (this.textarea.value = "", t.draft = "");
			});
		}), this.panel.append(this.composer), this.shell.append(this.rail, this.panel), e.replaceChildren(this.shell);
	}
	render() {
		if (this.disposed) return;
		let e = this.controller.state, t = e.selectedChannelId ? e.channels.find((t) => t.id === e.selectedChannelId) ?? null : null;
		this.renderRail(e), this.renderHeader(e, t), this.renderWarnings(e), this.renderHistory(e), this.olderButton.textContent = e.loadingOlder ? "Loading older messages…" : "Load older messages", this.olderButton.disabled = !e.hasMore || e.loadingOlder || e.loading, this.patchComposer(e), this.panel.setAttribute("aria-label", t ? `#${t.slug} conversation` : "Conversation");
	}
	dispose() {
		this.disposed = !0, this.root.replaceChildren();
	}
	renderRail(e) {
		this.rail.replaceChildren();
		let t = document.createElement("h2");
		t.textContent = "Channels", this.rail.append(t);
		let n = document.createElement("ul");
		n.className = "channel-list", n.setAttribute("aria-label", "Conversation channels");
		for (let t of e.channels) {
			let r = document.createElement("li"), i = document.createElement("button");
			i.type = "button", i.className = "channel-button", i.dataset.channelId = t.id, i.setAttribute("aria-pressed", String(t.id === e.selectedChannelId)), i.textContent = `#${t.slug}`, i.addEventListener("click", () => {
				this.controller.selectChannel(t.id);
			}), r.append(i), n.append(r);
		}
		this.rail.append(n);
		let r = document.createElement("button");
		if (r.type = "button", r.className = $.secondary, r.textContent = "Create Channel", r.addEventListener("click", () => {
			this.controller.toggleCreate();
		}), this.rail.append(r), e.creating) {
			let e = document.createElement("form");
			e.className = `channel-create-form ${$.field}`;
			let t = document.createElement("input");
			t.id = "new-channel-slug", t.name = "slug", t.className = $.input, t.required = !0, t.maxLength = 48, t.pattern = "[a-z0-9-]+", t.placeholder = "channel-name", t.setAttribute("aria-label", "New Channel name");
			let n = document.createElement("button");
			n.type = "submit", n.className = $.primary, n.textContent = "Create", e.addEventListener("submit", (e) => {
				e.preventDefault(), this.controller.createChannel(t.value);
			}), e.append(t, n), this.rail.append(e), queueMicrotask(() => t.focus());
		}
	}
	renderHeader(e, t) {
		this.headerSlot.replaceChildren();
		let n = document.createElement("header");
		n.className = "conversation-header";
		let r = document.createElement("h2");
		if (r.textContent = t ? `#${t.slug}` : "Conversations", n.append(r), t && t.canHide && !e.confirmingHide) {
			let e = document.createElement("button");
			e.type = "button", e.className = `${$.secondary} teamarness-danger`, e.textContent = "Hide Channel", e.addEventListener("click", () => {
				this.controller.showConfirmHide();
			}), n.append(e);
		}
		let i = document.createElement("p");
		i.className = "availability", i.setAttribute("role", "status"), i.textContent = `Status: ${e.availability}`, n.append(i), this.headerSlot.append(n);
	}
	renderWarnings(e) {
		for (let e of this.panel.querySelectorAll(".warning,.hide-confirmation")) e.remove();
		if (e.confirmingHide) {
			let e = document.createElement("div");
			e.className = `${$.noticeDanger} hide-confirmation`, e.setAttribute("role", "alertdialog"), e.setAttribute("aria-label", "Confirm permanent Channel hide");
			let t = document.createElement("p");
			t.textContent = "Hiding this Channel is permanent. There is no restore path.";
			let n = document.createElement("button");
			n.type = "button", n.className = `${$.secondary} teamarness-danger`, n.textContent = "Permanently Hide", n.addEventListener("click", () => {
				this.controller.hideChannel();
			});
			let r = document.createElement("button");
			r.type = "button", r.className = $.secondary, r.textContent = "Cancel", r.addEventListener("click", () => {
				this.controller.cancelConfirm();
			}), e.append(t, n, r), this.panel.insertBefore(e, this.history);
		}
		if (e.warning) {
			let t = document.createElement("p");
			t.className = `${$.noticeDanger} warning`, t.setAttribute("role", "alert"), t.textContent = e.warning, this.panel.insertBefore(t, this.history);
		} else if (!e.confirmingHide && e.warnings[0]) {
			let t = document.createElement("p");
			t.className = `${$.noticeDanger} warning`, t.setAttribute("role", "alert"), t.textContent = e.warnings[0], this.panel.insertBefore(t, this.history);
		}
	}
	renderHistory(e) {
		let t = this.history.scrollHeight - this.history.scrollTop - this.history.clientHeight < 40;
		this.history.replaceChildren();
		for (let t of e.messages) {
			let e = document.createElement("li");
			e.className = "message";
			let n = document.createElement("button");
			n.type = "button", n.className = "message-author";
			let r = t.isLocal;
			n.textContent = Ri(t.author, void 0, r), n.title = t.author, n.setAttribute("aria-label", `Show full Public Identity ${t.author}`), n.addEventListener("click", () => {
				n.textContent = t.author;
			});
			let i = document.createElement("div");
			i.className = "message-markdown", i.innerHTML = Ii(t.markdown), e.append(n, i), this.history.append(e);
		}
		t && (this.history.scrollTop = this.history.scrollHeight);
	}
	patchComposer(e) {
		let t = e.selectedChannelId !== null && !e.loading && e.availability !== "offline";
		this.sendButton.disabled = !t, this.textarea.placeholder = e.selectedChannelId ? "Write a message…" : "Select a Channel to write a message.";
	}
};
//#endregion
//#region src/index.ts
async function Bi(e) {
	let t = f(e.root), n = !1, r = new h(e.client, () => {
		n || i.render();
	}), i = new zi(e.root, r);
	await r.initialize(), i.render();
	let a = await e.client.subscribe("conversations.changes", {}, async (e) => {
		n || (await r.applySnapshot(e), i.render());
	});
	return async () => {
		n || (n = !0, r.dispose(), i.dispose(), await a.close(), t());
	};
}
//#endregion
export { Bi as mount };
