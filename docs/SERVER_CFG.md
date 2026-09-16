# Server.cfg Compatibility & Parsing Specification

Project Aldivine treats `server.cfg` as a first-class configuration format, preserving compatibility with existing FiveM and FXServer server scripts.

## 1. Supported Directives

- **Host & Project Identification**:
  - `sv_hostname "<name>"`: Display name in the server directory
  - `sv_projectName "<name>"`: Project title
  - `sv_projectDesc "<desc>"`: Detailed server description
  - `sv_maxclients <count>`: Maximum simultaneous client connections
  - `sv_tags "<tag1, tag2>"`: Discovery keywords
  - `sv_locale "<locale>"`: Primary server language (e.g. `en-US`, `id-ID`)
  - `sv_gameBuild <build>`: Target GTA V build number (e.g. 2699, 3095)
  - `sv_icon "<path>"`: Server icon PNG file

- **Networking & Endpoints**:
  - `endpoint_add_tcp "<ip:port>"`: TCP control & HTTP listener bind
  - `endpoint_add_udp "<ip:port>"`: AstraNet UDP datagram listener bind

- **Resource Lifecycle**:
  - `ensure <resource>` / `start <resource>`: Start a resource package on boot
  - `stop <resource>`: Stop an active resource
  - `restart <resource>`: Hot-restart an active resource

- **Variables & Convars**:
  - `set <name> <value>`: Server-local convar
  - `setr <name> <value>`: Replicated convar visible to connected clients
  - `sets <name> <value>`: Public directory metadata tag
  - `set_secret <name> <spec>`: Vault / environment secret reference

- **Access Control & Permissions**:
  - `add_ace <principal> <object> <allow|deny>`: Access control entry
  - `remove_ace <principal> <object> <allow|deny>`: Remove access rule
  - `add_principal <child> <parent>`: Principal inheritance hierarchy
  - `remove_principal <child> <parent>`: Remove inheritance link

- **File Inclusion**:
  - `exec <relative_path>`: AST-preserving file inclusion with cycle detection

## 2. Normalization

Both `server.cfg` and `server.toml` parse into `NormalizedServerConfig` in `crates/ald-servercfg`, enforcing uniform validation across all server runtimes.
