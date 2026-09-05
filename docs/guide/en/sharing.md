+++
title = "Showing a site to your phone"
slug = "sharing"
order = 8
summary = "Put one site on the local network, scan a QR code, and take it back off again — one site, one port, one rule."
+++

# Showing a site to your phone

Everything MixEngine serves answers on loopback and nowhere else. Testing on a real phone means
making an exception, and the exception is per site, deliberate, and reversible.

```bash
mix site share blog.test
```

That prints a URL your phone can open and a QR code you can point a camera at. Three things
happened:

1. The site started answering on this machine's address on the local network, instead of only on
   loopback. **This site only** — every other site keeps answering on loopback alone.
2. The certificate was reissued to cover that address, so the padlock survives the trip.
3. One administrator prompt asked for a firewall rule, for that one port.

## When the machine has more than one network

MixEngine **refuses to choose** rather than putting your site on a network you did not mean — a
laptop on office Wi-Fi and a VPN at the same time is the case this exists for. It names the
candidates, and you pick:

```bash
mix site share blog.test --interface "Wi-Fi"
```

## A share that ends by itself

```bash
mix site share blog.test --for 2h
```

`30s`, `90m`, `2h`, `1d`, or a bare number of seconds. The length is measured **from when the share
began**, so asking for a length shorter than the site has already been shared for is refused rather
than ending it on the spot.

Without `--for`, a share lasts until you end it or until this machine leaves the network it was
shared on. That last case is worth knowing about: closing the laptop lid and opening it somewhere
else ends the share, because the address it was shared at is not this machine's address any more.

## Taking it back

```bash
mix site unshare blog.test
```

That removes the firewall rule, rebinds the site to loopback, and reissues the certificate without
the network address. A site that is not shared is left exactly as it is, so running it when you are
not sure costs nothing.

## What to know before you use it

- **Anybody on that network can reach the site.** There is no authentication in front of it. On a
  café network or a shared office, that is the whole story — share for a length, and unshare when
  you are done.
- **The certificate is still MixEngine's.** Your phone does not trust MixEngine's authority, so it
  will warn. Sharing is for checking a layout on a real screen, not for demonstrating a padlock.
- **Nothing about your other sites changes.** The rule is one port, one site, and it is undone by
  `unshare`, by the length running out, or by leaving the network.

Everything the prompt asks for is listed in
[What MixEngine asks permission for](./permissions.md).
