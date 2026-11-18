# Play anywhere

[Rubylight docs](README.md) › [Play anywhere](README.md#play-anywhere) · [Play over the internet: how it works](internet-play.md) · [Troubleshooting](troubleshooting.md#clients-outside-your-network-cannot-connect)

At home a paired device finds the PC by itself. Away from home it needs a way in. Pair the device at home first and check that a stream works there; every way below then reuses that pairing.

## Choose a way in

| Way in | What you get | What it costs | Use it when |
| --- | --- | --- | --- |
| [Tailscale or another VPN](#tailscale-or-another-vpn) | The PC as if you were at home, from any network, for every client. Nothing on the PC is open to the internet. | An app and an account on the PC and on each device. A relayed route adds delay. | You can install an app on the device. This is the recommended way. |
| [Automatic port forwarding](#automatic-port-forwarding) | Moonlight and Nightjar reach the PC at your public address. | The streaming ports are open to the internet; the router must allow UPnP, PCP or NAT-PMP. | Your router has a public address and you can't use a VPN. |
| [Play over the internet](#play-over-the-internet) | Nightjar reaches the PC even where no port can be forwarded, such as behind carrier-grade NAT. | Nightjar only. The device learns the way once on your home network. Two routers that both change ports per destination can't be passed. | Port forwarding is impossible and you use Nightjar. |
| [Invite a friend over the internet](multiclient.md#invite-a-friend-over-the-internet) | A friend with Nightjar joins your game from a one-time link, with no PIN and no port forwarding. | Guest rights only, for a day by default; you are asked before they join. | You want to share your game with a friend who uses Nightjar. |
| [Guests in the browser](browser-guests.md) | A friend watches or plays along from a link, with no app and no pairing. | The guest ports are reachable from where the friend is; **Invite → On the internet** asks your router to forward them. | You want to share your game with someone else, not play your own. |

## Tailscale or another VPN

1. Install [Tailscale](https://tailscale.com/download) on the PC and on the device, and sign in to the same account on both.
2. Away from home, turn Tailscale on in the device and connect to the PC's Tailscale address, or let Nightjar find it.

Rubylight treats a Tailscale connection as your own network: it needs no port forwarding and Tailscale encrypts the stream. Nightjar's **Play away from home** guide on each PC walks through the same steps and tests the connection from another network ([Nightjar: Play away from home](../client/docs/remote-setup.md)). With Rubylight, Nightjar also shows whether the route is **direct** or goes through a Tailscale **relay**; a relay works but adds delay. Any other VPN that puts the device on your home network works the same way.

## Automatic port forwarding

**Automatic port forwarding** (**All settings → Network → Discovery and internet**, off by default) asks your router to forward the streaming ports while Rubylight runs and removes them when it stops. It also opens the ports in the router's IPv6 firewall where the router allows that. **Settings → Network → Internet status** shows what the router did with each port, your public address, and why a port was refused.

- **What it costs.** The streaming ports answer anyone on the internet. Only paired devices can stream, and streams that leave your network are encrypted whenever the device supports it (**Internet encryption**, on by default; a device that can't encrypt still streams, with a warning in the console).
- **When to change it.** Leave it off when you use a VPN. Turn it on when your router has its own public address and no VPN fits. When the console says the router's address is not public (carrier-grade NAT), port forwarding can't help; use Tailscale or Play over the internet.
- **Forwarding by hand.** A router without UPnP, PCP or NAT-PMP can forward the streaming ports by hand; **Internet status** lists them ([Clients outside your network cannot connect](troubleshooting.md#clients-outside-your-network-cannot-connect)). Never forward TCP 47990, the console's port.

## Play over the internet

**Play over the internet** (**All settings → Network → Discovery and internet**, off by default, applies after a restart) lets Nightjar reach the PC when no port can be forwarded. The PC and the device find each other through a public peer-to-peer directory that holds only encrypted notes, and open a direct path through both routers. No server of ours or anyone else's carries the stream ([how it works](internet-play.md), [privacy](privacy.md#third-party-services)).

1. Turn it on and restart Rubylight.
2. Open Nightjar once on your home network, so the device learns the way.
3. Away from home, Nightjar tries your network, the PC's IPv6 address, a forwarded port and then the direct path, in that order. The PC's line names the path in use, or why each one failed.

Leave it off if every device you use away from home has a VPN.

## Wake the PC from away

Wake-on-LAN packets don't cross the internet by themselves. To wake the PC from away, keep a device at home that can send the wake packet, such as a router with a Wake-on-LAN page or another always-on computer, or leave the PC on and let **Sleep after idle** put it to sleep when you are done ([Sleep and wake](everyday-use.md#sleep-and-wake)).

## Keep the console private

The console and its API answer only on your local network by default (**Console access**). Leave it that way: to change a setting from away, connect through your VPN. A tunnel or reverse proxy in front of the console makes internet connections look local, so don't put one there ([Clients outside your network cannot connect](troubleshooting.md#clients-outside-your-network-cannot-connect)).
