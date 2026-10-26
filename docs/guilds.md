# Guild reconstruction

Target 123.5.1 supplies the native creation, invitation, Chronicle, Board, rank and merchant interfaces. Its recovered packet definitions and rank utility take precedence over newer servers. The readable 7.0 client explains these flows through GuildUtil, GuildPlayerList, GuildBoardWindow and GuildMerchant. Comparator pins and licensing caveats are in the gameplay-reference documentation; shared ancestry is not independent corroboration.

Period evidence distinguishes production rules from temporary test settings:

- https://wikiwiki.jp/rmd/Guilds describes creation, membership, ranks, contributions and halls.
- https://wikiwiki.jp/rmd/test/build120.7 records early guild testing; its reduced creation price is not the production price.
- https://wikiwiki.jp/rmd/version/build122.0.1 documents Founder exceptions and guild earnings from member deaths.
- https://wikiwiki.jp/rmd/test/build121.5.2 and https://wikiwiki.jp/rmd/version/build123.4.0 provide upgrade and invitation-era context.

Creation spends 1,000 personal account Fame. Membership belongs to the account, with five ranks and at most 50 members. The selected client's matrix lets Officers change Initiate/Member, Leaders manage lower ranks and appoint Leaders, and Founders manage lower ranks including Leaders. There is no automatic Founder succession. Native help advertises `/invite`, `/join` and `/guild`; `/g` is also accepted.

Personal spendable Fame, personal lifetime earnings, guild bank, guild lifetime earnings and member contributions are separate. Death credits the guild in the permanent-death transaction. Hall upgrades spend 10,000, 100,000 and 250,000 guild Fame, require Leader or Founder authority and validate the actual merchant and current hall level.

Board reads and Chronicle requests require authenticated membership; Board editing requires Officer rank. Invitations revalidate authority and capacity at acceptance. Hall admission revalidates membership at arrival. Revoked occupants leave through durable Nexus transfer. Occupied halls and outstanding arrival grants retain their layouts; empty halls use normal five-minute cleanup before replacement.

Reconstruction policies: invitations expire after 60 seconds; comparator teleport settings disagree, so hall teleport remains disabled. The four converted layouts are comparator candidates, not authenticated official 123.5.1 maps. See the data attribution. No modern potion NPC, shared bank, guild wars or later redesign is inferred.
