---
title: Preparing installer media
description: The URL to bake into each ISO — and why its path decides which documents may answer.
sidebar:
  label: Installer media
  order: 3
---

# Preparing installer media

Every installer is told, at build time, where to fetch its configuration. That URL does
two jobs here:

1. **It reaches the server.** Any path works — `POST` and `GET` are answered on all of
   them, precisely so the URL baked into an ISO is never wrong.
2. **Its path declares the format.** A segment naming a known alias restricts the answer
   to documents of that format, so a kickstart client cannot be handed TOML.

Give each installer its own URL, and one server can answer for all of them.

## Proxmox VE

```console
$ proxmox-auto-install-assistant prepare-iso proxmox-ve.iso \
    --fetch-from http \
    --url http://SERVER:8000/proxmox/answer \
    --output proxmox-auto.iso
```

The installer POSTs a JSON inventory of the hardware it found and expects the answer in
the response body. `/proxmox/` restricts the reply to `.toml` documents; `/answer` on its
own names no alias and constrains nothing, which is why an existing deployment keeps
working unchanged.

To require a credential, prepare the ISO with a token and give the server the same one:

```console
$ proxmox-auto-install-assistant prepare-iso proxmox-ve.iso \
    --fetch-from http --url http://SERVER:8000/proxmox/answer \
    --answer-auth-token 'a-long-random-string' --output proxmox-auto.iso

$ export RESCRIPTUM_ANSWER_TOKEN='a-long-random-string'
```

See [Security](./operations/security.md) for what that protects and what it does not.

### Without rebuilding the ISO

Proxmox can also discover the URL at boot, which saves rebuilding media when the address
changes:

- a DNS TXT record on `proxmox-auto-installer.<your-domain>`, or
- DHCP option 250.

Both are outside this server's scope — it only has to be at the address they name.

## Everything else, over iPXE

Other installers *fetch* their configuration, and identify themselves in the query string
because iPXE substitutes its own variables into the URL before fetching:

| Variable | Is |
|---|---|
| `${net0/mac}` | the first NIC's MAC address |
| `${uuid}` | the SMBIOS system UUID |
| `${serial}` | the system serial number |
| `${manufacturer}`, `${product}` | DMI vendor and model |

Those become [facts](./answers/selection.md#the-facts-a-selector-can-test) a document can
be selected on, and their values also feed the substring haystack — so a document named
after a MAC resolves whether the MAC arrived in a POST body or a query string.

| Installer | Boot parameter |
|---|---|
| **RHEL / CentOS / Fedora / Alma / Rocky** | `inst.ks=http://SERVER:8000/rhel/ks?mac=${net0/mac}` |
| **Debian preseed** | `url=http://SERVER:8000/debian/preseed?mac=${net0/mac}` |
| **Ubuntu autoinstall** | `autoinstall ds=nocloud-net;s=http://SERVER:8000/ubuntu/?mac=${net0/mac}` |
| **Flatcar / Fedora CoreOS** | `ignition.config.url=http://SERVER:8000/flatcar/config?mac=${net0/mac}` |
| **openSUSE / SLES** | `autoyast=http://SERVER:8000/suse/profile?mac=${net0/mac}` |
| **Windows** | fetched by your own tooling from `http://SERVER:8000/windows/unattend` |

A full iPXE script fragment:

```
#!ipxe
set base http://SERVER:8000
kernel ${base}/images/rhel9/vmlinuz inst.ks=${base}/rhel/ks?mac=${net0/mac}&serial=${serial}
initrd ${base}/images/rhel9/initrd.img
boot
```

rescriptum serves the answer, not the kernel — netbooting stays with whatever TFTP/HTTP
server you already run.

### Ubuntu and cloud-init NoCloud

cloud-init's NoCloud datasource fetches **two** named files from the seed URL —
`user-data` *and* `meta-data` — and skips the datasource entirely if either is missing.
Since this server answers on any path, both requests would otherwise receive the same
document and the install would never start.

The path's last segment is available as the `file` fact, so the two are told apart with a
selector:

```yaml
# answers/groups/ubuntu-web/ubuntu.yaml
match:
  file: "user-data"
  product: "PowerEdge R6*"
```

```yaml
# answers/groups/ubuntu-meta/ubuntu.yaml
match:
  file: "meta-data"

instance-id: iid-local01
```

Note the trailing slash in `s=http://SERVER:8000/ubuntu/` — cloud-init appends the file
name to it.

NoCloud can also expand `__dmi.chassis-serial-number__` into the seed URL, which puts the
machine's identity in the *path* rather than the query. Path segments feed the haystack
too, so a document named after that serial still resolves.

## Choosing the alias

| URL segment | Serves documents with extension |
|---|---|
| `proxmox`, `pve`, `toml` | `.toml` |
| `debian`, `preseed` | `.preseed`, `.seed` |
| `rhel`, `centos`, `fedora`, `alma`, `rocky`, `kickstart`, `ks` | `.ks` |
| `ubuntu`, `autoinstall`, `cloudinit`, `nocloud`, `yaml`, `yml` | `.yaml`, `.yml` |
| `flatcar`, `coreos`, `ignition`, `ign` | `.ign`, `.json` |
| `suse`, `opensuse`, `autoyast` | `.autoyast`, `.xml` |
| `windows`, `unattend` | `.unattend`, `.xml` |
| `json` | `.json`, `.ign` |
| `xml`, `cfg`, `ipxe` | the matching extension |

Any segment of the path may name the alias, so `/rhel/ks`, `/ks`, and
`/provision/rhel/node.cfg` all restrict to kickstart. A URL naming none of them —
`/answer` — constrains nothing.

The full table, and why `seed` is deliberately **not** an alias, is in the
[format reference](./reference/formats.md).

## Next

- [How an answer is picked](./answers/selection.md) — what the server does with what the
  URL just told it.
- [One document per operating system](./answers/formats.md) — the same machine, as
  Proxmox and as Debian, at the same time.
