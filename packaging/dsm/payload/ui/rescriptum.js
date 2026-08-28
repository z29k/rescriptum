/*
 * rescriptum — the DSM desktop application.
 *
 * DSM's own UI framework, not a page of ours in a frame. `SYNO.SDS.AppInstance`,
 * `SYNO.SDS.AppWindow` and the `syno_*` widgets are what the desktop provides, so the
 * window is a real DSM window, in the DSM theme, in the DSM language.
 *
 * **ExtJS rather than Vue, and that is the machine's doing, not a preference.** DSM 7.2
 * ships a Vue framework and Synology's current guide documents only that one — but the
 * DS416j this project exists for is capped at DSM 7.1.1, where `Vue` is simply undefined.
 * ExtJS is on both (measured, 7.1.1 and 7.2.2), so one application covers every DSM this
 * package supports rather than two covering one each.
 *
 * The API is documented, in the ExtJS reference Synology generated for DSM — mirrored at
 * https://github.com/DigitalBox98/SimpleExtJSApp as `docs/synoextjsdocs.tar.gz`, with a
 * worked "Basic application how to".
 *
 * **That guide's own example does not run.** It declares classes with `Ext.define` and
 * chains with `callParent`, and against `SYNO.SDS.AppInstance` on DSM 7.2.2 that throws
 * `Cannot read properties of null (reading 'apply')` before the window ever appears: this
 * is **ExtJS 3.4.1** (`Ext.version` says so), the SYNO classes are built with `Ext.extend`,
 * and the `Ext.define` shim cannot find a parent constructor to chain to. `Ext.extend`
 * plus `superclass.constructor.call` is what works, and it is what DSM's own code uses.
 *
 * Written by hand and shipped as it is read — no bundler, nothing from `node_modules` in
 * the release. Everything it shows comes from `api.cgi`, which is where the authentication
 * is, and the field list comes from `rescriptum config --json`: adding a variable to the
 * server does not mean editing this file.
 */
Ext.ns('SYNO.SDS.App.Rescriptum');

(function () {
    'use strict';

    /* Substituted by make-spk.sh, and it is not cosmetic — see the cache note below. */
    var VERSION = '@VERSION@';
    var BASE = '/webman/3rdparty/rescriptum/';
    var API = BASE + 'api.cgi';

    /* The two variables with a fixed set of answers. Everything else is free text, which
     * is what the server itself says about them — a menu here is a nicety, not a rule, and
     * a variable this file has never heard of still gets a field. */
    var CHOICES = {
        RESCRIPTUM_STORE: ['files', 'sqlite'],
        RESCRIPTUM_LOG: ['all', 'problems', 'off']
    };

    /* **DSM does not load a hand-written package's texts, and this is where that was
     * found.** `ui/config` declares `"texts": "texts"`, the files are served — a request
     * for `texts/fre/strings` answers 200 — and DSM's own `_T('ui', 'settings')` still
     * comes back empty. Presumably the desktop loads them for packages built with
     * Synology's toolchain, which registers rather more than a config file.
     *
     * So the application loads them itself. What stays Synology's is everything that
     * matters: the file format, the `[section]` layout, and the locale directory names,
     * which come straight from `_S('lang')` — `fre` on a French DSM. Nothing here is a
     * string table in JavaScript; a translation is still a file.
     *
     * The same goes for `ui/config`, whose `title` and `desc` are literals rather than
     * `section:key` references: an unresolved reference shows up as the literal text
     * `app:description` under the icon. */
    function parseStrings(text) {
        var strings = {};
        var section = '';
        Ext.each(String(text || '').split('\n'), function (raw) {
            var line = raw.replace(/^\s+|\s+$/g, '');
            if (!line || line.charAt(0) === '#') { return; }
            var header = /^\[(.+)\]$/.exec(line);
            if (header) { section = header[1]; return; }
            var at = line.indexOf('=');
            if (at < 0) { return; }
            var key = line.slice(0, at).replace(/^\s+|\s+$/g, '');
            var value = line.slice(at + 1).replace(/^\s+|\s+$/g, '').replace(/^"([\s\S]*)"$/, '$1');
            strings[section + ':' + key] = value;
        });
        return strings;
    }

    /* `Ext.define` for the declaration — DSM's launcher finds the class that way, and it
     * sets `superclass` correctly — but **never `callParent`**, which is where the guide's
     * example falls over. See the note at the top. */
    Ext.define('SYNO.SDS.App.Rescriptum.AppInstance', {
        extend: 'SYNO.SDS.AppInstance',
        appWindowName: 'SYNO.SDS.App.Rescriptum.AppWindow'
    });

    Ext.define('SYNO.SDS.App.Rescriptum.AppWindow', {
        extend: 'SYNO.SDS.AppWindow',

        constructor: function (config) {
            var self = this;

            this.strings = {};
            this.settings = [];
            this.writable = true;

            /* Components are built and kept, rather than looked up by itemId afterwards.
             * `getComponent` and card layouts disagree about what an itemId is often
             * enough that holding the reference is simply cheaper than being right. */
            this.settingsPanel = new SYNO.ux.FormPanel({
                border: false,
                autoScroll: true,
                labelWidth: 220,
                bodyStyle: 'padding: 12px 16px',
                items: []
            });
            /* **A form panel, not a plain one.** `syno_displayfield`'s `fieldLabel` is
             * drawn by the *form* layout; in a plain `Ext.Panel` the labels silently do
             * not render and the status page comes out as a bare column of values with
             * nothing saying what they are. Found on the DS416j, where the settings tab
             * looked right — it was a form already — and this one did not. */
            this.statusPanel = new SYNO.ux.FormPanel({
                border: false,
                autoScroll: true,
                labelWidth: 220,
                bodyStyle: 'padding: 12px 16px',
                items: []
            });
            this.logPanel = new Ext.Panel({
                border: false,
                autoScroll: true,
                bodyStyle: 'padding: 12px 16px',
                html: ''
            });
            /* Plain, not a form: this one is a list with buttons rather than labelled
             * fields, so the form layout that `statusPanel` needs would buy nothing. */
            this.mediaPanel = new Ext.Panel({
                border: false,
                autoScroll: true,
                bodyStyle: 'padding: 12px 16px',
                items: []
            });

            this.tabButtons = {
                settings: new SYNO.ux.Button({ text: 'Settings', toggleGroup: 'rescriptum-tabs', allowDepress: false, pressed: true, handler: function () { self.showView('settings'); } }),
                status: new SYNO.ux.Button({ text: 'Status', toggleGroup: 'rescriptum-tabs', allowDepress: false, handler: function () { self.showView('status'); } }),
                media: new SYNO.ux.Button({ text: 'Images', toggleGroup: 'rescriptum-tabs', allowDepress: false, handler: function () { self.showView('media'); } }),
                log: new SYNO.ux.Button({ text: 'Log', toggleGroup: 'rescriptum-tabs', allowDepress: false, handler: function () { self.showView('log'); } })
            };
            this.saveButton = new SYNO.ux.Button({ text: 'Save', handler: function () { self.save(); } });
            this.reloadButton = new SYNO.ux.Button({ text: 'Reload', handler: function () { self.reload(); } });
            this.closeButton = new SYNO.ux.Button({ text: 'Close', handler: function () { self.close(); } });

            /* **The stack of views is a panel inside the window, not the window's own
             * layout.** `SYNO.SDS.AppWindow` arranges its own chrome, and overriding
             * `layout` on it gives a window that opens, gets a taskbar entry and a preview
             * — and renders empty. One `fit` child that owns the card layout leaves the
             * window's own arrangement alone. */
            this.deck = new Ext.Panel({
                border: false,
                layout: 'card',
                activeItem: 0,
                items: [this.settingsPanel, this.statusPanel, this.mediaPanel, this.logPanel]
            });

            config = Ext.apply({
                /* **DSM's taskbar calls `getWindowTitle()` on the window.** Without a
                 * title it throws `t.button.getWindowTitle is not a function` from inside
                 * the taskbar bundle, and the app then fails to open at all — with the
                 * error pointing at DSM's code rather than at ours. */
                title: 'rescriptum',
                width: 880,
                height: 660,
                resizable: true,
                maximizable: true,
                minimizable: true,
                layout: 'fit',
                tbar: [this.tabButtons.settings, this.tabButtons.status, this.tabButtons.media, this.tabButtons.log],
                items: [this.deck],
                buttons: [this.saveButton, this.reloadButton, this.closeButton]
            }, config);

            SYNO.SDS.App.Rescriptum.AppWindow.superclass.constructor.call(this, config);

            /* Strings first, so nothing is ever painted with a raw key in it, then the
             * configuration the form is made of. */
            this.loadStrings(function () { self.loadConfig(); });
        },

        /* Belt to the `title` config's braces: the taskbar wants this method, and a
         * window that has been retitled at runtime should still answer. */
        getWindowTitle: function () {
            return this.title || 'rescriptum';
        },

        // ---- text ------------------------------------------------------------

        t: function (key) { return this.strings['ui:' + key] || key; },
        label: function (key) { return this.strings['label:' + key] || key; },
        help: function (key) { return this.strings['help:' + key] || ''; },

        loadStrings: function (done) {
            var self = this;
            var lang = 'enu';
            try { lang = (window._S && _S('lang')) || 'enu'; } catch (e) { /* enu it is */ }

            var read = function (which, then) {
                Ext.Ajax.request({
                    /* The version is on the URL for the same reason it is in this file's
                     * name: every packaged file has a fixed mtime so builds are
                     * reproducible, nginx serves that as `Last-Modified: 2019`, and a
                     * browser's heuristic freshness is then years. */
                    url: BASE + 'texts/' + which + '/strings?v=' + encodeURIComponent(VERSION),
                    method: 'GET',
                    success: function (response) { then(parseStrings(response.responseText)); },
                    failure: function () { then({}); }
                });
            };

            /* English underneath, always: a key translated in one file and not the other
             * falls back to a sentence rather than to a blank label. */
            read('enu', function (english) {
                if (lang === 'enu') {
                    self.strings = english;
                    self.relabel();
                    done();
                    return;
                }
                read(lang, function (translated) {
                    Ext.iterate(translated, function (k, v) { if (v) { english[k] = v; } });
                    self.strings = english;
                    self.relabel();
                    done();
                });
            });
        },

        /* The chrome exists before the strings arrive, so it is labelled once they do. */
        relabel: function () {
            this.tabButtons.settings.setText(this.t('settings'));
            this.tabButtons.status.setText(this.t('status'));
            this.tabButtons.media.setText(this.t('media'));
            this.tabButtons.log.setText(this.t('log'));
            this.saveButton.setText(this.t('save'));
            this.reloadButton.setText(this.t('reload'));
            this.closeButton.setText(this.t('close'));
        },

        // ---- talking to api.cgi ----------------------------------------------

        /* A write must prove it was made by our own application rather than by a page
         * somewhere else that happens to be open in the same browser: a browser will not
         * send an invented header cross-origin without a preflight first, and `api.cgi`
         * answers no preflight. DSM's own SynoToken goes along too, which is what keeps
         * this working with DSM's cross-site request forgery protection turned on. */
        call: function (action, options) {
            options = options || {};
            var self = this;
            var headers = { 'X-Rescriptum': '1' };
            try {
                var token = window._S && _S('SynoToken');
                if (token) { headers['X-SYNO-TOKEN'] = token; }
            } catch (e) { /* api.cgi has its own guard */ }

            var request = {
                url: API + '?action=' + encodeURIComponent(action) + (options.query || ''),
                method: options.body === undefined ? 'GET' : 'POST',
                headers: headers,
                timeout: 60000,
                success: function (response) { options.success(response.responseText); },
                failure: function (response) {
                    /* api.cgi answers a refused write with the server's own sentence —
                     * "this would leave a server that cannot start …" — which is the most
                     * useful thing this window can put in front of somebody. */
                    var text = ((response && response.responseText) || '').replace(/^\s+|\s+$/g, '');
                    self.banner(text || ('HTTP ' + (response && response.status)));
                }
            };
            /* `jsonData` given a string is how this Ext sends a body verbatim; `params`
             * would form-encode it, and the body here is KEY=VALUE lines that api.cgi
             * hands to the CLI one argument at a time. The Content-Type it sets is not
             * CORS-safelisted either, which only helps. */
            if (options.body !== undefined) { request.jsonData = options.body; }

            Ext.Ajax.request(request);
        },

        // ---- settings ---------------------------------------------------------

        loadConfig: function () {
            var self = this;
            this.call('config', {
                success: function (text) { self.applyConfig(Ext.decode(text)); }
            });
        },

        applyConfig: function (payload) {
            var self = this;
            var form = this.settingsPanel;

            this.settings = payload.settings || [];
            this.envFile = payload.env_file || '';
            this.writable = payload.writable !== false;

            form.removeAll(true);

            this.bannerField = new SYNO.ux.DisplayField({ hidden: true, hideLabel: true, htmlEncode: true, cls: 'rescriptum-banner' });
            this.restartButton = new SYNO.ux.Button({ hidden: true, text: this.t('restart'), handler: function () { self.restart(); } });
            form.add(this.bannerField);
            form.add(this.restartButton);

            form.add(new SYNO.ux.DisplayField({
                hideLabel: true, htmlEncode: true, cls: 'rescriptum-envfile',
                value: this.t('env_file') + ' ' + (this.envFile || this.t('env_file_none'))
            }));

            if (!this.writable) {
                form.add(new SYNO.ux.DisplayField({ hideLabel: true, htmlEncode: true, cls: 'rescriptum-banner', value: this.t('read_only') }));
            }
            if (payload.starts === false && payload.error) {
                form.add(new SYNO.ux.DisplayField({ hideLabel: true, htmlEncode: true, cls: 'rescriptum-banner', value: this.t('would_not_start') + ' ' + payload.error }));
            }
            Ext.each(payload.warnings || [], function (warning) {
                form.add(new SYNO.ux.DisplayField({ hideLabel: true, htmlEncode: true, cls: 'rescriptum-banner', value: warning }));
            });

            this.fields = {};
            Ext.each(this.settings, function (setting) {
                var field = self.fieldFor(setting);
                self.fields[setting.key] = field;
                form.add(field);

                var help = self.help(setting.key) || setting.help;
                if (help) {
                    form.add(new SYNO.ux.DisplayField({ hideLabel: true, htmlEncode: true, cls: 'rescriptum-help', value: help }));
                }
                if (setting.source === 'environment') {
                    form.add(new SYNO.ux.DisplayField({ hideLabel: true, htmlEncode: true, cls: 'rescriptum-help', value: self.t('from_environment') }));
                }
            });

            form.doLayout();
            this.saveButton.setDisabled(!this.writable);
        },

        fieldFor: function (setting) {
            /* A value the environment sets cannot be changed by editing the file, so the
             * field is shown and disabled rather than hidden: pretending the file is the
             * whole story is how somebody edits a value for an hour and then wonders why
             * the server ignores it. */
            var common = {
                name: setting.key,
                fieldLabel: this.label(setting.key),
                disabled: setting.source === 'environment',
                width: 340
            };

            if (CHOICES[setting.key]) {
                var rows = [];
                Ext.each(CHOICES[setting.key], function (value) { rows.push([value]); });
                return new SYNO.ux.ComboBox(Ext.apply({
                    mode: 'local',
                    triggerAction: 'all',
                    editable: false,
                    forceSelection: true,
                    valueField: 'value',
                    displayField: 'value',
                    store: new Ext.data.ArrayStore({ fields: ['value'], data: rows }),
                    value: setting.value || ''
                }, common));
            }

            /* A secret is never sent down, so its box is always empty — and an empty box
             * therefore means "leave it alone", never "clear it". Clearing a token is
             * deliberate enough to be worth a shell. */
            if (setting.secret) {
                return new SYNO.ux.TextField(Ext.apply({
                    inputType: 'password',
                    value: '',
                    emptyText: setting.set ? this.t('secret_set') : this.t('secret_unset')
                }, common));
            }

            return new SYNO.ux.TextField(Ext.apply({
                value: setting.value === null ? '' : setting.value,
                emptyText: setting['default'] || ''
            }, common));
        },

        save: function () {
            var self = this;
            var lines = [];

            Ext.each(this.settings, function (setting) {
                if (setting.source === 'environment') { return; }
                var field = self.fields[setting.key];
                if (!field) { return; }
                var now = field.getValue();
                now = (now === undefined || now === null) ? '' : String(now);

                if (setting.secret) {
                    if (now) { lines.push(setting.key + '=' + now); }
                    return;
                }
                var was = setting.value === null ? '' : String(setting.value);
                if (now !== was) { lines.push(setting.key + '=' + now); }
            });

            if (!lines.length) {
                this.banner(this.t('nothing_changed'));
                return;
            }

            this.call('save', {
                body: lines.join('\n') + '\n',
                success: function (text) {
                    self.applyConfig(Ext.decode(text));
                    self.banner(self.t('restart_needed') + ' ' + self.t('restart_closes'));
                    self.restartButton.show();
                    self.settingsPanel.doLayout();
                }
            });
        },

        banner: function (message) {
            if (!this.bannerField) { return; }
            this.bannerField.setValue(message);
            this.bannerField.show();
            this.settingsPanel.doLayout();
        },

        /* Restarting is DSM's job, not the package's. `api.cgi` runs as the package user
         * and has no privilege to start or stop anything — and a process it started would
         * land outside the package's cgroup, where DSM could no longer stop it.
         * SYNO.Core.Package.Control does it properly, with the signed-in administrator's
         * own session, and `getBaseURL` is what puts DSM's own credentials on the
         * request. */
        restart: function () {
            var self = this;
            var step = function (method, then) {
                Ext.Ajax.request({
                    url: self.getBaseURL({ api: 'SYNO.Core.Package.Control', method: method, version: 1 }),
                    method: 'POST',
                    params: { id: 'rescriptum' },
                    timeout: 120000,
                    success: function (response) {
                        var payload = Ext.decode(response.responseText, true);
                        if (!payload || payload.success !== true) {
                            self.banner('DSM refused to ' + method + ' the package.');
                            return;
                        }
                        then();
                    },
                    failure: function () { self.banner('DSM refused to ' + method + ' the package.'); }
                });
            };
            step('stop', function () {
                step('start', function () {
                    self.banner(self.t('restarted'));
                    self.loadStatus();
                });
            });
        },

        // ---- status and log ---------------------------------------------------

        loadStatus: function () {
            var self = this;
            var panel = this.statusPanel;
            this.call('status', {
                success: function (text) {
                    panel.removeAll(true);
                    /* `key: value` lines from a shell script, given their labels here.
                     * Both halves are looked up and both fall back to what was sent, so a
                     * line the CGI grows before this file hears about it still shows —
                     * untranslated rather than missing. */
                    Ext.each(String(text).split('\n'), function (line) {
                        if (!line) { return; }
                        var at = line.indexOf(': ');
                        var name = at < 0 ? line : line.slice(0, at);
                        var value = at < 0 ? '' : line.slice(at + 2);
                        panel.add(new SYNO.ux.DisplayField({
                            htmlEncode: true,
                            fieldLabel: self.strings['status:' + name] || name,
                            value: self.strings['value:' + value] || value
                        }));
                    });
                    panel.doLayout();

                    self.call('check', {
                        success: function (report) {
                            panel.add(new Ext.Panel({
                                border: false,
                                html: '<pre class="rescriptum-pre">' + Ext.util.Format.htmlEncode(report) + '</pre>'
                            }));
                            panel.doLayout();
                        }
                    });
                }
            });
        },

        loadLog: function () {
            var self = this;
            this.call('log', {
                query: '&lines=200',
                success: function (text) {
                    self.logPanel.update('<pre class="rescriptum-pre">' + Ext.util.Format.htmlEncode(text) + '</pre>');
                }
            });
        },

        // ---- the three views ---------------------------------------------------

        // ---- images ----------------------------------------------------------

        /* Three sections, and the order is the order somebody works in: what is held,
         * what can be fetched, and the manual way in that must never disappear. */
        loadMedia: function () {
            var self = this;
            var panel = this.mediaPanel;
            panel.removeAll(true);
            panel.add(new Ext.Panel({ border: false, html: '<div class="rescriptum-pre">' + self.t('loading') + '</div>' }));
            panel.doLayout();

            this.call('media', {
                success: function (text) {
                    panel.removeAll(true);
                    panel.add(self.section(self.t('held'), text));

                    /* The catalogue picker. The source list is local and instant; asking
                     * one what it offers goes over the network to the vendor, so it is a
                     * second click rather than something done for every source on open. */
                    self.sourceCombo = new SYNO.ux.ComboBox({
                        fieldLabel: self.t('source'),
                        width: 260,
                        editable: false,
                        triggerAction: 'all',
                        mode: 'local',
                        valueField: 'id',
                        displayField: 'label',
                        store: new Ext.data.ArrayStore({ fields: ['id', 'label'], data: [] })
                    });
                    self.offerCombo = new SYNO.ux.ComboBox({
                        fieldLabel: self.t('image'),
                        width: 420,
                        editable: false,
                        triggerAction: 'all',
                        mode: 'local',
                        valueField: 'name',
                        displayField: 'name',
                        store: new Ext.data.ArrayStore({ fields: ['name'], data: [] })
                    });
                    self.sourceCombo.on('select', function (c, rec) { self.loadOffers(rec.get('id')); });

                    var fetchBtn = new SYNO.ux.Button({
                        text: self.t('fetch'),
                        handler: function () { self.fetchImage(); }
                    });
                    var urlField = new SYNO.ux.TextField({ fieldLabel: self.t('url'), width: 420 });
                    var digestField = new SYNO.ux.TextField({ fieldLabel: self.t('digest'), width: 420 });
                    self.urlField = urlField;
                    self.digestField = digestField;

                    panel.add(new SYNO.ux.FormPanel({
                        border: false, labelWidth: 120, bodyStyle: 'padding: 4px 0 12px 0',
                        items: [
                            new Ext.Panel({ border: false, html: '<b>' + Ext.util.Format.htmlEncode(self.t('catalogue')) + '</b><div style="margin:2px 0 8px 0">' + Ext.util.Format.htmlEncode(self.t('catalogue_hint')) + '</div>' }),
                            self.sourceCombo, self.offerCombo, fetchBtn
                        ]
                    }));

                    /* **The manual way in, and it is not a lesser path.** A digest somebody
                     * obtained out of band is stronger evidence than one read from the same
                     * host as the image — so this stays in front of people rather than being
                     * documented as an escape hatch for the command line. */
                    panel.add(new SYNO.ux.FormPanel({
                        border: false, labelWidth: 120, bodyStyle: 'padding: 4px 0 12px 0',
                        items: [
                            new Ext.Panel({ border: false, html: '<b>' + Ext.util.Format.htmlEncode(self.t('manual')) + '</b><div style="margin:2px 0 8px 0">' + Ext.util.Format.htmlEncode(self.t('manual_hint')) + '</div>' }),
                            urlField, digestField,
                            new SYNO.ux.Button({ text: self.t('add'), handler: function () { self.addByUrl(); } })
                        ]
                    }));

                    self.progressPanel = new Ext.Panel({ border: false, html: '' });
                    panel.add(self.progressPanel);
                    panel.doLayout();
                    self.fillSources();
                    self.pollProgress();
                }
            });
        },

        section: function (title, text) {
            return new Ext.Panel({
                border: false,
                html: '<b>' + Ext.util.Format.htmlEncode(title) + '</b>' +
                      '<pre class="rescriptum-pre">' + Ext.util.Format.htmlEncode(text) + '</pre>'
            });
        },

        /* `media sources` prints a table; the ids are its first column. Parsed here
         * rather than served as JSON so the panel and the command line stay the same
         * text — the rule the `config` action already follows. */
        fillSources: function () {
            var self = this;
            this.call('sources', {
                success: function (text) {
                    var rows = [];
                    Ext.each(String(text).split('\n'), function (line) {
                        var m = /^([a-z0-9][a-z0-9._-]*)\s\s+(\S.*?)\s\s+/.exec(line);
                        if (m && m[1] !== 'SOURCE') { rows.push([m[1], m[2]]); }
                    });
                    if (self.sourceCombo) { self.sourceCombo.getStore().loadData(rows); }
                }
            });
        },

        loadOffers: function (id) {
            var self = this;
            this.offerCombo.getStore().loadData([]);
            this.offerCombo.setValue('');
            this.banner(this.t('reading_index'));
            this.call('sources', {
                query: '&source=' + encodeURIComponent(id),
                success: function (text) {
                    var rows = [];
                    Ext.each(String(text).split('\n'), function (line) {
                        var m = /^ {2}(\S+\.(?:iso|img))\s*$/i.exec(line);
                        if (m) { rows.push([m[1]]); }
                    });
                    self.offerCombo.getStore().loadData(rows);
                    if (rows.length) { self.offerCombo.setValue(rows[0][0]); self.banner(''); }
                    else { self.banner(text); }
                }
            });
        },

        fetchImage: function () {
            var self = this;
            var src = this.sourceCombo && this.sourceCombo.getValue();
            var name = this.offerCombo && this.offerCombo.getValue();
            if (!src || !name) { this.banner(this.t('pick_one')); return; }
            this.call('fetch', {
                query: '&source=' + encodeURIComponent(src) + '&name=' + encodeURIComponent(name),
                body: '',
                success: function () { self.banner(''); self.pollProgress(); }
            });
        },

        addByUrl: function () {
            var self = this;
            var url = (this.urlField && this.urlField.getValue() || '').replace(/^\s+|\s+$/g, '');
            var digest = (this.digestField && this.digestField.getValue() || '').replace(/^\s+|\s+$/g, '');
            if (!url) { this.banner(this.t('need_url')); return; }
            this.call('fetch', {
                query: '&url=' + encodeURIComponent(url) + '&sha256=' + encodeURIComponent(digest),
                body: '',
                success: function () { self.banner(''); self.pollProgress(); }
            });
        },

        /* Progress is the partial file's size, which `media add` is already writing —
         * nothing had to be invented for the browser, and nothing here can disagree with
         * what the CLI actually did. */
        pollProgress: function () {
            var self = this;
            this.stopPolling();
            var tick = function () {
                self.call('progress', {
                    success: function (text) {
                        if (!self.progressPanel) { return; }
                        self.progressPanel.update('<pre class="rescriptum-pre">' + Ext.util.Format.htmlEncode(text) + '</pre>');
                        if (/^state: running/m.test(text)) {
                            self.pollTimer = setTimeout(tick, 2000);
                        } else {
                            self.pollTimer = null;
                            /* Finished: the held list has changed, so re-read it once
                             * rather than leaving a stale table in front of somebody. */
                            if (self.active === 'media' && self.sawRunning) {
                                self.sawRunning = false;
                                self.loadMedia();
                            }
                        }
                        if (/^state: running/m.test(text)) { self.sawRunning = true; }
                    }
                });
            };
            tick();
        },

        stopPolling: function () {
            if (this.pollTimer) { clearTimeout(this.pollTimer); this.pollTimer = null; }
        },

        /* **Not `show`.** `Ext.Window.prototype.show()` is what DSM calls to display the
         * window, and defining a method of that name here silently overrode it: the window
         * was built, laid out and even rendered its taskbar preview, and then never
         * appeared — because "showing" it ran this tab-switcher instead. Nothing threw, on
         * either DSM version. Anything added to this prototype shares a namespace with
         * every method of `Ext.Window`, and that is a large namespace. */
        showView: function (which) {
            var panel = this.settingsPanel;
            if (which === 'status') { panel = this.statusPanel; }
            if (which === 'log') { panel = this.logPanel; }
            if (which === 'media') { panel = this.mediaPanel; }
            this.deck.getLayout().setActiveItem(panel);
            this.active = which;
            this.saveButton.setDisabled(which !== 'settings' || !this.writable);
            if (which === 'status') { this.loadStatus(); }
            if (which === 'log') { this.loadLog(); }
            if (which === 'media') { this.loadMedia(); }
            /* Polling only while the tab is in front. A timer left running behind a
             * closed window is a request every two seconds, forever, on a NAS. */
            if (which !== 'media') { this.stopPolling(); }
        },

        reload: function () {
            if (this.active === 'status') { this.loadStatus(); return; }
            if (this.active === 'log') { this.loadLog(); return; }
            if (this.active === 'media') { this.loadMedia(); return; }
            this.loadConfig();
        }
    });
})();
