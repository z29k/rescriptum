# Kickstart — RHEL, CentOS Stream, Fedora, AlmaLinux, Rocky.
#
# Layering here is CONCATENATION, not a merge: a machine file's directives follow these
# rather than replacing them. Note that ordinary comments like this one ARE served; only
# `# answer:` lines are stripped.
#
#   rescriptum render --query "serial=7ABC123"
#
# answer: match serial=7ABC*

lang fr_FR.UTF-8
keyboard --xlayout=fr
timezone Europe/Paris --utc

# openssl passwd -6
rootpw --iscrypted $6$rounds=656000$REPLACE$ME
sshkey --username=root "ssh-ed25519 AAAA...REPLACE ops@example.com"

network --bootproto=dhcp --device=link --activate
firewall --enabled --service=ssh
selinux --enforcing

clearpart --all --initlabel
autopart --type=lvm

reboot

%packages
@^minimal-environment
qemu-guest-agent
%end
