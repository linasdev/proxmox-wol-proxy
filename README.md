# Proxmox Wake-on-LAN proxy
This tool lets you manage your Proxmox node, via Wake-on-LAN and the Proxmox API, based on the number of incoming requests.

## Usage
This is meant to be put after a reverse proxy but in front of the services running on your Proxmox node.
*If you do not put a reverse proxy in front of this service, attackers will be able to execute arbitrary HTTP requests using your hardware*.

### Example setup
WAN ↔ Nginx (on the proxy host) ↔ Proxmox Wake-on-LAN proxy (on the proxy host) ↔ Proxmox node (on a separate host)

## License
This software is licensed under the [MIT](./LICENSE.md) license.
