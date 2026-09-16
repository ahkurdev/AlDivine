# Aegis Setup & First-Run Provisioning

## 1. First-Run Bootstrap

When `ald-server` or `aegis-node` is launched on an unconfigured server:
1. Aegis Node Agent binds strictly to localhost (`127.0.0.1:40120`). Never binds to public interfaces by default.
2. A temporary single-use owner bootstrap token is printed to stdout.
3. The operator navigates to `http://127.0.0.1:40120` to complete the 15-step setup wizard.

## 2. Setup Wizard Steps

1. Welcome
2. Create Owner Account (Username, strong password, TOTP 2FA)
3. New Server vs Import Existing FiveM Server
4. Server Identity (Name, description, tags, locale, game build)
5. Gameplay Framework Selection (Aldivine Framework, ESX, QBCore, Qbox, Standalone)
6. Database Configuration (PostgreSQL, MySQL, MariaDB, SQLite Dev)
7. Network & Ports (AstraNet UDP port, HTTP/TCP port)
8. Player Capacity & Slots (max players, reserved slots)
9. Base Resources Selection (spawn, chat, session, etc.)
10. Initial Admin Accounts & Roles
11. Security Controls (rate limits, firewall, anti-cheat tolerances)
12. Review Configuration
13. Transactional Deployment
14. Health Check
15. Dashboard Redirect
