## Reference material

Prefer Trapped (older numeric IDs), realm-src (readable states), FSoD (coverage); compare feature-specific engine/XML semantics. 

| Pin | Use/caveats | License |
| --- | --- | --- |
| https://github.com/trapped/rotmg_svr/tree/593e490803fe0ad168ead408cacd5287d778acfe | 2013 comparator | none |
| https://github.com/moistosaurus/realm-src/tree/1855c017eab870d13f0b61b1cf0b9e66a01922ff | Modified NR-Core 7.0; unfinished dungeons/server-side hit tracking | GPL-3.0; check AGPL integration |
| https://github.com/moistosaurus/realm-cli/tree/a13c855dc5e09ee301fd0a66fcd303cfb5dff74e | Modified 7.0 client, not AI/2012 specification | none |
| https://github.com/ossimc82/fabiano-swagger-of-doom/tree/6fd20aad4a7905b13f25389c68368a942a2b68cb | 27.3.2; README warns about 27.5 | AGPL-3.0 + attribution/named-user exclusions: unresolved |
| https://github.com/Bledixon/Project-B-Public/tree/9947199c8d384cc8220657bfa7519dc92e47adb9 | Seraph-derived; custom items/elders/level-120; compare normal variants | AGPL-3.0 + credits |
| https://github.com/Devwarlt/Rotux/tree/8156148a303c0fdcb783d34276ddbf4130ebca4e | FSoD-based | server/LICENSE: AGPL-3.0; inherited terms unresolved |
| https://github.com/Omniraptyr/Valor-Server/tree/40fab9e80d8a44e922036d178b9d1f03aca66a44 | Later/custom secondary comparator, not 123.5.1 specification; four boss state-machines: `VotR-Server/wServer/logic/db` | AGPL-3.0: `VotR-Server/LICENSE` (root detector returned null) |
| https://github.com/Devwarlt/LoE-Realm-NC-2/tree/bc11dff7e76fc9444f417dd761d7ba01f3de13c2 | Heroes-of-Oryx bosses; custom client 3/2.12 | MIT; inherited rights unverified |
| https://github.com/cp-nilly/NR-CORE/tree/070977917893a5fbb6fe9598e5d63ff8c62fa10c | 27.7.X13; excludes assets/enemy behaviors; infrastructure only | AGPL-3.0 |
| https://github.com/cp-nilly/nillys-realm-phoenix/tree/e5eb61dddd921ef605fbcd09c7b352f4af9504e6 | Phoenix-Realms ancestry; unmaintained | none |
| https://github.com/Club559/ROTMGServer/tree/f112a71caa5e73f3013d9bff88527669519f178c | Resolve active logic/db versus logicUpd/db registration/build | none |
| https://github.com/Club559/Seraphs-Dominion-Public/tree/3b3a9e06e0521ffabdb76390c2dc84f76239eeec | Custom crafting/scaling/currency | none |
| https://github.com/Zolmex/creepylava-source/tree/a44a6e72dd85d0116ff34a819eddeaadb1bb96b4 | 2024 upload ≠ development date; ancestry unauthenticated | none |
| https://github.com/Zolmex/mmoempire-v3-source/tree/b21be11b3a8715b727e3d85e2ee8460ae1f9b762 | Preservation comparator | none |
| https://github.com/Zolmex/kithio-as3-source/tree/d15f321edcce0ef890001890c6991c25a1b0f718 | Includes C# server tables | none |
| https://github.com/kaos00723/betterSkillys/tree/069d4816f45d151c7df14be84d0d04ee11d31307 | TK-Reborn; 1.0.4.0 XML/Bard/Samurai/Exalt maps, not 2012 | MIT; inherited source/map rights unverified |

Boss sample: Ent Ancient, Lich, Ghost King, Phoenix Lord. All four tables were located in 13 server sources; realm-cli is client-only, NR-CORE removes the behavior library, and betterSkillys received version/lineage triage only—not verified four-boss coverage. Line-ending-normalized matches: Rotux↔FSoD (four); Club559 logic/db↔Trapped (four), logicUpd/db↔Kithio (four); Seraph↔Project-B (Ghost/Phoenix); MMOEmpire↔Trapped (Ghost/Lich/Phoenix). **Not independent corroboration.**

Discovery: https://docs.realmdex.com/sources.html (“Skillys V1?”→realm-src/realm-cli). Hidden “Skilly's 7.0 NR-Core”/“Educational” links remain unidentified; don't equate with betterSkillys. Zero.Gdk returned 404; no verdict/pin.

## Implementation contract

Pin feature/engine/XML/registration anchors. Selected client identities/stats/projectiles and bounded populations/native authority/permanent death/committed once-only rewards remain authoritative. Separate reference behavior, historical corroboration, conflicts and reconstruction. Review file-level licenses/notices even for Rust translations; no bundled binaries, stack imports or source/map/client dumps. Deterministic phase/reward/lifecycle tests then native multiplayer verify implementation, not historical fidelity. No hosting/restoration-score/next-feature scope change.
