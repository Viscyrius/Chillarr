# Changelog

## 0.20.2 (2026-11-12)
- Fixed for good: picking Dubbed, or English in the player's audio menu, really plays English now. Jellyfin was ignoring the choice and played the file's first track (Japanese), and Chillarr never told it which copy of the file it meant. VLC never had this problem because it picks the track itself.
- Much less CPU while watching. When your PC (or phone) can play the file's own picture format, AV1 included, Chillarr now has Jellyfin only repackage it instead of converting the picture. Mushoku Tensei season 3 went from about 40% CPU to almost nothing. Subtitles are no longer drawn into the picture either; Chillarr shows them itself.
- With "Use graphics card for video" on, the card now also decodes AV1 and VP9 where it can (RTX 30 and newer, Intel Arc, Radeon RX 6000 and newer), for the files that still need converting.
- Chillarr rests when nobody is using it. With no one watching, reading or tapping for 15 minutes, it only looks for new films, episodes and chapters and finishes downloads; everything else waits, and wakes the moment someone starts. Jellyfin is no longer asked about every profile's library every 30 seconds, and the window hidden in the tray stops redrawing itself.
- Velvet: the arrows and dots under the featured title work again (the rows were covering them).

## 0.20.1 (2026-11-12)
- Velvet is its own style now, not Cinema in other colours. Watching is a stage: the featured title stands in the middle between velvet curtains, its poster in a brass frame, rows read like a printed programme, and every card is a ticket with a torn stub. Reading is a reading room: what you're reading lies open on a desk with a ribbon marking how far you are, and everything else stands on shelves.
- The filters' adult-only choice is now called 18+ R.
- In Velvet each kind of reading looks like what it is: a manhwa or manhua is a tall scroll in a phone frame, a manga is a paperback with page edges, a novel or book is a hardcover with a cloth spine, and a comic is a stapled issue.

## 0.20.0 (2026-11-12)
- Fixed for good: Dubbed now means English voices are really in the file. A release only counts as an English dub when its name says English. Plain "Dual Audio" is no longer taken as proof, and Portuguese dubs ("Dublado", "PT-BR") are refused like other foreign dubs. That is how Mushoku Tensei came in with Japanese and Portuguese only. The rule that refuses foreign dubs had broken characters in Sonarr and Radarr and refused nothing. It is checked and rebuilt now, and when Chillarr can't change it, the log says so.
- With Dubbed on, Chillarr checks each downloaded episode and film for the dub language. A file without it is replaced when a copy that names it turns up. The same release is never taken twice, and nothing is deleted until a replacement has been found.
- The player starts in English when you chose Dubbed, also when the track is only named "English" without a language tag. When the file has no English voices, it says so. A language you picked yourself for a show still wins. Jellyfin's own apps now start in your dub language too.
- Fixed for good: Hide really hides. Hidden titles never show on any page again, and they have their own Hidden tab. The Hidden tab opens with your hidden password, only on the device you typed it on, for 15 minutes. Before, setting the password or unlocking showed every hidden title on every page, on every device of that profile, for an hour that kept starting over. That is why hiding seemed to do nothing.
- The main profile, in the Chillarr app on the PC, can reset any profile's hidden password with one button in Manage profiles. What they hid stays hidden.
- Fixed: a film and a show with the same TMDB number could hide each other.
- Fixed: reading showed chapters from the wrong place as the newest ones, like Swordborne's "Chapter 276" (a different manga) and Reincarnated Murim Lord's 559 and 560 (old chapters with stretched pages). Chillarr now checks that a chapter number fits the rest of that copy's list. These strays had also stopped the real next chapters from downloading by themselves, and that works again.
- Pages in AVIF format show at the right size instead of stretched.
- Reading search finds a series when you type its name with an extra "s" or a letter off ("Chronicles of Runes").
- Activity now counts reading, and phones. It used to look only at Jellyfin plays, so someone reading manhwa on their phone for two days showed as "Last on 2 days ago". It also shows what each profile read last, and "Reading now".
- Phones can delete downloaded books and chapters. Any profile can delete what only it reads; the main profile can delete anything, and is asked first when someone else reads it too.
- The chapter range in a series ("Chapters 1 to 20") now has Download and Delete next to Mark read.
- Chapter downloads move live, without the page redrawing itself. Before, the series panel was rebuilt every 5 seconds, and the Downloads tab every 4, which made pictures blink, moved the scroll and cleared what you were typing. A finished movie download no longer redraws the reading pages either. When the manga downloader can't be reached, the Downloads tab says so instead of "Nothing is downloading".
- Midnight's Latest updates shows two rows, newest first. A series with a new chapter slides in and the oldest drops off the end.
- New episodes at the top of Home, Series and Anime: every show you watch that has episodes out since you last watched it, with the newest one, how many are new and how long ago. Home shows the count on its tab.
- My List has a New for you section at the top: shows on your list with new episodes, films on your list that are ready to watch, and in reading, the series you follow with new chapters.
- Comments on every reading title (manga, manhwa, manhua, novels, books and comics), like a film's. A series has one list, whichever site you opened it from.
- Read Party has a chat, like Watch together's. On a computer it sits to the right of the page and the page makes room for it, so it never covers what you're reading. On a phone it's a small sheet at the bottom that shrinks to a button with one tap. Messages popping up while you read can be turned off.
- Reading clean-up, When finished: the clock starts once every downloaded chapter is marked read. Chapters you never downloaded don't hold it back any more. Both choices now say exactly what they do, with your number of days.
- New style: Velvet. Burgundy velvet with brass when dark, claret on rose paper when light, with serif titles. Pick it under the Look button, for watching and reading.
- Custom colours: next to Dark and Light, Custom lets you pick the page, card, text and accent colours for any style. The page changes as you drag.
- Fixed: opening another title while a show was still loading could put the show's buttons, Delete included, on the other title's page.
- Fixed: a crash or blue screen while saving settings could wipe every setting and show setup again. Settings are now saved safely, with the last good copy kept.
- Fixed: in Webtoon mode, a page that failed to load stayed broken until you reopened the chapter. It now loads again when you scroll back to it.
- Fixed: Watch together gave up reconnecting after one failed try (after the PC slept, for example), and the party went quiet while still showing you as in it. It keeps trying now.
- Fixed: a subtitle from the last episode could show up on the next one.
- Fixed: the book reader could show an error page as if it were the chapter.

## 0.19.2 (2026-11-11)
- Reading search finds series AniList doesn't have, and series you know by another name. It now asks MangaUpdates too: Sword God High School: The Talentless Underachiever Becomes the Strongest shows up (AniList doesn't list it at all), and Chronicle of Runes finds Emblem Wars, which is the same manhwa. A series only MangaUpdates knows can be followed and read like any other.
- Fixed: in reading, clearing the search box sometimes left the search results on screen instead of going back to the tab you were on, so you had to search something else and clear it again.

## 0.19.1 (2026-11-10)
- Fixed: switching to another profile on a phone (most often an iPhone) could sign you out of Chillarr completely and leave you on Jellyfin's username and password page. The iPhone keyboard sent the PIN twice, Chillarr started two switches, and Jellyfin's second sign-in ended the first one, which was the one the phone kept. A PIN is now sent once, a switch never runs twice at once, and the switch keeps trying through a bad connection instead of giving up. If a phone ever does end up holding an ended sign-in after a switch, Chillarr puts the right one back by itself.
- Fixed: on phones and in the browser there was no way to see your hidden things again, because the Hidden settings were only in the Chillarr app on the PC. They're now in Settings > Account on every device: Show hidden (with your hidden password), Lock, and the list of what's hidden.
- The hidden list now has Open next to each title, which takes you straight to its page. This works for anything hidden from now on; titles hidden before only have Show again.
- Fixed: other profiles couldn't remove things they'd asked for. Stop on Downloading now and the waiting list said they had no permission, so their titles sat on My List > On the way for good. Every profile can now stop what it asked for itself (and still only that).
- Other profiles can now delete a title from the library if they downloaded it themselves or it's on their My List. Delete shows on that title's page, on phones too. Everything else is still the main profile's to delete, in the Chillarr app on the PC.
## 0.19.0 (2026-11-10)
- Fixed: picking another style (Cinema, Midnight or Classic) on a reading tab only changed the colours and kept the old layout. It now redraws the page in the new style, like the watching tabs always did.
- New colour: Dusk, a warm peach over deep violet. Pick it under the Look button or Settings > Appearance.
- Up to 12 profiles now (it was 8).
- The main profile, in the Chillarr app on the PC, runs every profile: open, edit, change or remove anyone's PIN, set their password, or remove them, without knowing their PIN.
- Manage profiles > Activity (main profile, on the PC only): when each profile was last on, on any device, what they're watching right now and on which device, and what they watched lately. Anyone not on for a month is marked Inactive, with Remove right there.
- Removing a profile now cuts off every way they had in at once: their Jellyfin account, their sign-ins on phones and TVs, and the phone remote codes for their devices.
- Privacy: other profiles no longer see what's downloading or waiting in general. Downloading now, On the way, the waiting list and download progress only show them what they asked for themselves, and they can only stop their own. What's in the library and ready to watch is unchanged. The main profile still sees everything.
## 0.18.1 (2026-11-10)
- Fixed: on a phone or in a browser, Chillarr went back to Jellyfin's old look by itself, usually about 20 seconds after a page finished loading, or when you came back to the app. 0.18.0 made this much worse. It now only goes back if Chillarr's page truly can't start, and then a "Back to Chillarr" button is right there.
- Fixed: when Jellyfin restarted (an update, or Chillarr starting it), phones were signed out and dropped on Jellyfin's old look, which is what kicked you out of a watch party. Now the page waits a few seconds for Jellyfin to come back and carries on. You're only sent to sign in if Jellyfin itself says your sign-in is gone.
- Chillarr no longer restarts Jellyfin for an update while anyone has used it in the last 10 minutes, not just while something is playing.
- After an update, a phone that still has the old page open keeps working until it's reloaded.
- If Chillarr's page ever does go back to Jellyfin's own, it notes why, and Chillarr's log says so the next time it opens.
- Faster on phones and in the browser: the app comes as one file instead of 27, pictures only load when they come into view (Home used to ask for 300 at once), series posters come in a size made for phones (a tenth of the size), what the PC sends is compressed, and a tab you opened a moment ago comes back at once.
- Deleting from your library (and reading downloads) is only on the main profile, and only in the Chillarr app on the PC. It's gone from other profiles and from phones.
- Settings > Library & storage: tick several titles (or Select all) and delete them in one go. They leave the list straight away and are deleted together. Deleting is for good, not to the Recycle Bin.
- Kids profiles: R and NC-17 films and Ecchi anime are kept out of every row, Similar and Watch now too, not only search and filters. A kids' profile in the Chillarr app can only make kids' profiles.
- Fixed: Series always showed the filtered grid with "Filters 1" instead of its rows.
- Fixed: "Back to all rows" in reading kept the grid and turned 18+ off.
- Two new colours: Sakura (soft pink) and Aurora (teal over deep indigo). Pick them under the Look button or Settings > Appearance.
- Every tab has a look of its own now: away from Home, Movies, Series, Anime, My List, Downloads and each reading tab (Books, Manga and the rest) get their own soft glow at the top, made from your colour, so it works with every theme.
- Reading tabs come back instantly when you switch back to them: Home and Books (Books used to take several seconds each time), and Manga, Manhwa, Manhua and Novels, right where you were in the list, still loading more as you scroll. Home and Books still show your progress and new chapters: they're redrawn after a few minutes, or straight away when something you read changes.

## 0.18.0 (2026-11-10)
- Kids profiles: when you add or edit a profile there's a Kids profile switch. A kids' profile only opens titles rated up to PG and never sees 18+, in filters, search or anywhere else. Only the main profile can turn it off again, and a kids' profile can only make kids' profiles.
- 18+ is on from the start for everyone else: R and NC-17 films and mature anime and manga show in search, filters and on a director's or writer's page without turning anything on. Porn is still its own choice and stays off until you pick it. You can still turn 18+ off in Filters.
- Watch parties: everyone in the party can find the R-rated film the host is playing now. It used to be hidden from them until they turned on 18+.
- Fixed: on a phone, Jellyfin's old look sometimes came back instead of Chillarr and stayed until you closed the app. A slow connection counted as Chillarr being broken; now it only falls back on a real error, and tries Chillarr again next time. A phone left open over an update also lost the page; it now moves to the new one.
- Fixed: a Watch now title that had finished downloading still played through the slow download stream, so it took a while to start (Ghost in the Shell). It now plays the downloaded file straight away and keeps your place.
- Videos never ask for more than the file's own quality, so a film with picture subtitles turned on doesn't come out bigger than the file itself. Much easier on weak Wi-Fi.
- Faster on phones and in the browser: the video player loads only when you play something, posters and pictures come in the size they're shown at (about half the data on Home), and the blur effects are gone on phones. Buttons are easier to tap, and the top bar no longer runs off the edge on small laptops and tablets.

## 0.17.1 (2026-11-10)
- Manga, Manhwa, Manhua and Novels open on rows now, like Movies: Top 10 today, Most popular, Top rated, Ongoing, New and rising, then a row for each genre (Action, Fantasy, Romance, Comedy, Drama and more). Every row's title and Explore all open the whole list. Pick a filter or another order and the matching titles take the rows' place; Back to all rows brings them back.
- Your week shows every show of yours that is still running, not only ones with an episode in Sonarr this week. Your shows are the ones you download, watch, put on My List or favourite. Each sits on the day it airs, with the episode out this week or the next one announced; a show between seasons says so ("Between seasons, usually Wednesdays"). Air days come from Sonarr, TVmaze and AniList. On Series and Anime it stays on the page with a short note when nothing of yours is running.
- My List rows open and close from their title too, not just the Show all link, in watching and reading.

## 0.17.0 (2026-11-10)
- Movies, Series and Anime have their filters right on the page, like the reading tabs. Filters and Order sit above the rows with the genre chips beside them; pick anything and the matching titles take the rows' place. Back to all rows (or Clear filters) brings the rows back. Home's Filters still opens them on their own page.
- Your week now shows on Series, Anime and Home in every style, not just Classic: your shows by the day their episodes come out, each card with the episode (S2 E7) and when it's out ("Out today", "Fri at 21:00"). It opens on today, or on the next day something of yours comes out.
- Pick several at once: a Select button on My downloads, and on your series and books in reading (Continue reading, Finished, Following, Your books). Tick the ones you want (Shift and a click ticks a run), then Delete them, Delete downloads, Stop following, Delete files or Remove, all in one go.
- Deleting episodes and chapters: Shift and a click ticks every episode or chapter between two.
- Reading: a series' writer and artist are links now, like a film's director. Each opens everything they wrote or drew, and the series page shows "More by" with their other series.
- Reading: every chapter says when it came out ("Released 11/09/26"), in the chapter list and when picking chapters.
- Reading Home has rows picked for you, like Movies' "Because you watched": "Because you read Solo Leveling" and "More Action for you", made from what you read. Classic shows them as Picked for you. Books shows "More by" the authors of your books.
- Reading rows work like Movies' rows: the arrows only show when there's more to scroll, and the row titles open the whole row (See all for your own rows).
- New look for the top bar: one Look button opens Cinema, Midnight and Classic (each with a small picture of itself) and Dark or Light. It used to squeeze the top bar so "My downloads" broke onto two lines, and put a Style row on top of every reading page.
- New chapters on reading Home is a slim strip across the page instead of a big box with a poster in it.
- Limit downloads now gives downloads 20% of your internet instead of 10% (manga and books share the same 20%). Off is still no limit, and "Don't limit what I start here" works as before.
- Fixed: Stop following sometimes looked like it did nothing, so you pressed it again. It now closes the page at once and says it's stopping, and it can't be pressed twice. The series leaves your list straight away; if the manga downloader is slow to let go, it's tried again in the background.
- Fixed: closing the Delete its chapters question with Esc counted as Keep them and stopped following anyway. Closing it now stops nothing.
- Fixed: a book you added (or deleted) while another one was being filed could vanish from Books or come back. The same for followed series while new chapters were being checked.
- Fixed: while chapters downloaded, a series' page redrew itself every 5 seconds and threw away what you were doing on it (picking chapters to delete, Change copy, typing a chapter number).
- Fixed: switching a series' translation and pressing Follow quickly followed the copy found for the old translation.
- Fixed: Next chapter said "You're up to date" when the next chapter was missing but later ones were downloaded. It now goes to the next one that's here.
- Fixed: pressing Next twice in the reader loaded the same chapter twice, and Next then Previous could land on the wrong one.
- Fixed: a chapter that failed to open closed the series page and left you on nothing.
- Fixed: Manga, Manhwa, Explore all and other long reading grids jumped back to the top after closing the reader or when a download finished.
- Fixed: Latest updates and Your week in reading only looked at the 16 series you read most lately, so one that updated today could be missing.
- Faster: the reading list loads all your series at once instead of one by one, the manga downloader keeps its connections, rows far down the page aren't drawn until you get near them, finding copies of a series isn't started twice, and reading tabs remember what AniList said for a few minutes.

## 0.16.1 (2026-11-09)
- Reader: tap the page (or open a new chapter) and the chapter you're in shows at the top for a moment, so you don't get lost.
- Read Party: "Following the host." has an x to hide it, and Read together has a switch to show it again. It stays hidden next time too.

## 0.16.0 (2026-11-09)
- Fixed: adding some chapters of a finished series with "Also get new chapters" on downloaded the whole rest of the series (picking 1 to 100 of Noblesse queued all 545). New chapters now means chapters the site posts after you follow a series.
- Reading: one button, Get new chapters, in place of Follow and Keep up to date. When it's on, new chapters are checked every few minutes and download with a note. Adding a series now gets only what you picked unless you tick "Also get new chapters".
- New: Notify me on any film, series, anime, light novel or book, even ones not out yet. You get a note when it comes out (a film at home, a new episode or season, a new volume, an author's new book), on the PC, phones and the TV. Turn on Get it too to download it as soon as it's out.
- New: comics and graphic novels in Books. Rows for popular, new, superheroes, Marvel, DC, Image and Dark Horse, search, and reading in the reader with your place kept.
- New: click a director's, creator's or author's name to see everything they made, ready to get, watch or read.
- New: Your week in the Classic style for series and anime: your shows by the day their episodes come out, with UP on today's.
- Movies, Series and Anime filters now look and work like the Books filters. 18+ is Not included, Included or Only 18+, and porn is its own choice under it (anime and reading). R and NC-17 films and ecchi titles stay out until 18+ is on, or until you pick them directly.
- Watch together and Read Party: the host can lock a party or give it a password, remove someone (they can't rejoin for 10 minutes), hand over host, and turn on Only I control. Read Party has Follow me, where everyone follows the host's scroll, and Free, with a Jump to the host button.
- Book and manga pages list their details under the description: genres, author and artist, status, chapters, volumes, the latest chapter and when it came out.
- My List is one row in Movies and Reading, with Show all to open it.
- Reader: the mouse no longer hides by itself while you read. Turn it on with the new mouse button in the top bar (or M) if you want it gone.
- Reader on phones: one reading direction button (tap to switch between Scroll, Right to left and Left to right) in place of three that squeezed the top bar, and the top and bottom bars fit the screen.
- A director's or anime staff member's page leaves out R and NC-17 films and ecchi anime unless 18+ is on, like the rest of the app. Their page and a series' details also load faster when TVmaze or Wikidata is slow.
- Fixed: if the manga downloader didn't answer when new chapters came out, those chapters were skipped for good. They're tried again next round.
- Watch party locks and Only I control can't be got around any more by writing a request in different letters.
- Stop on a series or anime asks whether to stop the whole series or just this download. The whole series stops every episode and nothing more is fetched.

## 0.15.1 (2026-11-08)
- Fixed: manga only searched a few sites. Every big manga site (Weeb Central, MangaDex, Mangakakalot, MangaFire and more) was being skipped as 18+ because it carries some adult titles, so only MANGA Plus and the manhua sites were asked. Black Clover showed 6 chapters and Darling in the Franxx none; now 16 sites have all 392 of Black Clover, 15 have Darling in the Franxx, and One Piece has all 1198. Only sites that are entirely 18+ wait for an 18+ search.
- Fixed: a sharp copy with chapters missing in the middle now gets them filled in from the fullest copy.
- New: Hidden. Every profile can hide films, series, anime, manga and books from all its pages, behind a password of its own (not the PIN). Press Hide on a title's page. Settings > Account & devices sets the password, shows hidden things until you lock them (or for an hour), and lists them to show again.
- Reading: hide a series from Your week (and Latest updates) on your profile, so whoever looks at the screen doesn't see it. Use the button on its card in Your week or "Hide from Your week" on its page. "Show in Your week" brings it back. It is still in your list and opens as usual.
- Many more manga sites: 73 now, up from 28. Every big English site in the list is searched, plus Comick Live, MangaBall, NiAdd, NovelCool and more.
- Fixed: a series a site lists without chapters (often a licensed one, like Darling in the Franxx from Seven Seas) made that whole site give up. It now tries the site's next match and the series' other names. Sites are searched 12 at a time so the slow ones don't hold up the rest.

## 0.15.0 (2026-11-08)
- New: Read Party. Read a series together, like Watch together for films. Press the people button in the reader to start one and share the code; friends join from Read together in the top bar. Everyone stays on the same chapter and page, whoever turns it. Joining a series you don't follow adds it to your list without downloading it again.
- Many more manga sites: Mangahere, MangaFox, Mangatown, MangaKatana, Manganato, MangaPill, MangaRead.org, Bato, Dynasty Scans and Manhwatop, plus MANGA Plus and Comikey from the publishers. Every series is looked for on all of them at once.
- Fixed: the list of manga sites moved, so a new PC couldn't set up any. It uses the new list now.
- Fixed: "No source has this one yet" for series that were there (Darling in the Franxx). A slow site no longer drops the copy it found, and if every site is slow, it says so and lets you try again.
- Fixed: Best could pick a site that has only the first and newest few chapters (MANGA Plus with One Piece), so chapters like 1192 couldn't be downloaded. Best now picks a copy that has them all.
- More books: TorrentDownload is added, and books are also searched by name on sites that don't file them as books (LimeTorrents, TorrentDownload and 1337x had many that were missed). Settings > Download sites shows which sites are searched for books, and lists the manga sites.
- Books: where you stopped is kept, and a book counts as finished at 90%. Read again starts over; Resume still goes on from where you stopped.
- The reader stays out of the way: its bars no longer pop up while you scroll. Move the mouse to the top or bottom, tap the middle or press Enter to bring them back. New Full screen button (or F).
- Manhua and 18+ series without a banner now show their cover sharp at the top and in the series panel, not blurred.
- Reading: delete a series' downloaded chapters (all or some) or a book's file without unfollowing, and see where reading lives and what takes the space in Settings > Library & storage.
- Watch party chat: turn off "Show messages while watching" and messages wait in the chat with a count on the chat button, instead of popping up over the movie.
- Watch party: closed the player by accident? Watch together now has Join back, which puts you at the party's spot, playing or paused like everyone else, without making anyone wait.

## 0.14.0 (2026-11-08)
- Switching Style changes the page at once, also on Movies, Series and Anime (it could wait for the whole page to load again first).
- A TV, phone or browser that typed its profile's PIN stays unlocked for 30 days, even when Chillarr restarts (it used to ask again after every restart). If one does need the PIN again, the PIN pad comes up instead of pages not loading.
- Search finds 18+ titles too, once 18+ is on (Include or Only in a reading tab's filters).
- New: Style in Movies too. Cinema, Midnight or Classic, each in dark or light, from the picker in the middle of the top bar. Movies and reading each keep their own style, for each profile.
  - Midnight: dark violet and dense, a shorter picture at the top, ratings on the covers, and a Popular ranking down the side (Top 10, Trending, Series).
  - Classic: the featured picture is a card inside the page, and the Top 10 is a numbered chart with the title and genre beside each cover.
- New: Explore all on reading rows. Click a row's title (Popular manhwa, Fantasy books and the rest) to see everything in it, with more loading as you scroll. Book rows too.
- Reading looks tidier:
  - The arrows on the big picture sit with the dots at the bottom right, so they never cover the title. Same in Movies.
  - The picture behind is sharper when the series has a banner.
  - New chapters is only as wide as what's in it, not a big empty panel.
  - Midnight: the Popular now grid lines up with its heading, and Latest updates is a proper grid.
  - Classic: the Top 10 is a numbered chart with small covers. Cards show the title, then the genre and length on one line. UP and NEW no longer sit on top of each other.
  - Lists that switch with tabs fade across instead of jumping.
- Light mode, everywhere: buttons, the details panel, the series panel and the hover cards no longer keep dark colours. The player and the reader stay dark.
- On a narrow window the Style picker sits at the top of the page, in one row.
- New: 18+ titles can also come from HentaiRead. It's only searched for 18+ titles.
- 18+ in the reading filters can be turned on from any profile (it's still off until you pick it).
- A TV, phone or browser signed in as a profile with a PIN now asks for that PIN first.
- Each profile has its own manga and books: what one follows doesn't show on another's, and following something already here doesn't download it again.
- A profile made on a TV or phone now shows in the PC's profile picker right away.
- Watch together no longer micro-stutters: tiny pauses aren't sent to the whole party any more, and small drifts are caught up gently instead of jumping.
- Fixed: a Watch now film could stay in Downloads after it finished (The Dark Knight did). A player that has said nothing for 10 minutes no longer counts as still watching.

## 0.13.4 (2026-11-08)
- New chapters are hard to miss now: a glowing New chapters section at the top of reading Home (in every style), a count on the Home tab, NEW on each new chapter in a series, and a short note when you open reading. New means out since you last read that series, so following something doesn't call its whole back catalogue new.
- Eight more manga sites: Thunder Scans and Manhua Plus (furthest ahead on long manhua: The Devil Butler goes to chapter 922 in English there, where the others stopped at 701), plus ManhuaUS, Top Manhua, Mangakakalot, Mangabat, MangaK and Zinmanga. They're added the next time the manga downloader starts. On a series you follow, Change copy lists them.
- New: Dark and Light for every reading style (the sun and moon next to Style). Each style remembers its own; Classic starts light, the others dark.
- New: Similar to this, under every series.
- Filters: a Themes row next to Genres (Isekai, Murim, Regression, Cultivation, Villainess and more), and 70 themes under More filters. AniList itself has 18 manga genres.
- The reading tabs take on your style: Midnight gets the ranking down the side, Classic a banner and a numbered top 5.
- Fixed: Midnight's smaller picture at the top went under the top bar on wide screens.
- Fixed: Classic's hover cards were washed out (light grey on white).

## 0.13.3 (2026-11-08)
- Manhwa, Manhua, Manga and Novels now open with the big featured picture too, showing that tab's trending series, with your filters and the grid under it.
- The Style picker shows on every reading tab, not only after visiting Home.
- The top bar's buttons move now: the film and book switch wiggles, Watch together's people hop, Full screen's corners spread, the Settings gear turns, and every one lifts on hover and gives a little when pressed. The People button no longer tips sideways on hover.

## 0.13.2 (2026-11-08)
- New: Style, in the middle of the top bar while reading. Pick how reading looks:
  - Cinema: big pictures and rows, like the films. Trending is now a Top 10 with the big numbers.
  - Midnight: dark violet and dense. The newest chapters of what you follow up front, each one a click from the reader, with how long ago it came out; a grid of what's popular with ratings and chapter counts; and a Popular ranking down the side (Trending, Top rated, Most read).
  - Classic: light and bright. A numbered Top 10 (Trending, Popular, Top rated), Your week (your series by the day their chapters come out, with UP on today's), Popular by genre, and New and ongoing.
  Each profile keeps its own style.
- New: hover cards on manga, manhwa and manhua covers, like the films': the cover opens out with the picture, rating, a few lines, Read or See chapters, and My List.
- New: filters on Manhwa, Manhua, Manga and Novels like the films': genres and tags in or out, status, rating, length and years. 18+ can be hidden, included or shown alone, only on the PC owner's profile (or a Jellyfin admin on the web); other profiles never see 18+ titles.
- More movement: covers rise in as they load, rankings and updates slide in, switching style fades across, and the tabs and buttons react when pressed.

## 0.13.1 (2026-11-08)
- New: Keep up to date, on any series you follow. Chillarr checks the site for new chapters every few minutes instead of every few hours. A site that has trouble is asked less often until it answers again.
- New: a notification on this PC when new chapters are ready to read, for every series you follow, and when another site gets further ahead than your copy (Change copy on the series switches to it).
- New: mark chapters read in one go: chapters X to Y, or the whole series. Marking moves your place forward too.
- New: delete a series when it's finished. Settings > Library & storage > Delete chapters after reading now has As you read or When finished: the whole series goes a few days after everyone reading it has read the last chapter.
- New: the reading pages open with a big featured series at the top, like Movies, with the cover tilted on the right.
- A series you don't follow shows its chapters as soon as you open it, with nothing ticked. Tick the ones you want, press Download all, or press Follow at the top to just get new ones.
- Following a series again picks the copy whose chapters you kept, so they're there to read straight away.
- The series page opens more smoothly, and the reader's bars are more solid over white pages.
- Sharpen pages now stops the moment a game goes full screen, instead of finishing the chapter it was on.
- Fixed: after the clean-up deleted chapters you'd read, new-chapter checks could download every chapter you hadn't picked.

## 0.13.0 (2026-11-08)
- New: Reading mode. The book button at the top switches Chillarr to reading, with its own tabs (Home, Manhwa, Manhua, Manga, Novels, Books, Downloads, My List) and its own search. The film button switches back. Each profile remembers which one it was in.
- New: pick the translation you want, then Best or a copy of your own choosing.
- New: pick which chapters to get, or get one chapter at a time from the series page. You can also choose whether new chapters come in by themselves.
- Best now mixes copies: chapters come from the sharpest copy, and the newest ones from whichever site is furthest ahead. You get the good quality without waiting on a site that locks its newest chapters.
- Manga downloads now stay inside Limit downloads, like everything else.
- The reader zooms in and out in every mode: the - and + at the top, the + and - keys, Ctrl and the mouse wheel, or a pinch. Each profile keeps its own zoom for each mode. Scroll now starts at the pages' real size instead of blowing them up.
- The reader is smoother: pages slide in the way the story reads, switching modes or chapters fades across, and the arrow keys scroll smoothly.
- New: Sharpen pages with your graphics card, in Settings > General. Each chapter you download is redrawn once at twice the detail, in the background, and the reader shows HD on those. It waits while a game is full screen. Turning it off deletes the sharper copies; your chapters stay. It only shows in the app on this PC.

## 0.12.1 (2026-11-08)
- Fixed: pressing Find copies while the manga downloader was still being set up could say it "didn't start". It now waits for the setup to finish.

## 0.12.0 (2026-11-08)
- New: Read. A new tab for manga, manhwa, manhua, books and light novels, on the PC, phones and the TV.
- Pick a series and Chillarr checks every site for it, then shows the copies sharpest and most complete first. Download one and every chapter comes in, new chapters too.
- The reader shows each page exactly as the site has it. Scroll for manhwa and manhua (one long strip, the next chapter follows on), Right to left for manga (two pages side by side on a wide screen), and Left to right.
- Books and light novels come through your download sites like films do. Books open in a reader with Dark, Sepia and Light and a text size, and PDFs open in your browser.
- Each profile keeps its own place, read chapters and Finished list. Settings > Library & storage can delete chapters a few days after everyone reading them has read them.
- The manga downloader is set up for you, hidden like the other apps. New installs get it during setup, and this PC gets it the first time Chillarr starts after updating.
- New: Comments on every title. Everyone on your server sees them, and you can edit or delete your own.
- New: Chat in Watch together. Press C, or the chat button, to talk while you watch. The video never pauses.
- New: Ecchi works as a genre for films and shows from your library too, and genres are listed A to Z everywhere.
- Fixed: the first sign-in could open a profile with a PIN. A locked profile now always asks who's watching.
- Fixed: downloads no longer go into your own Downloads folder. They go to a Chillarr Downloads folder on the same drive as your library, and what was already downloading moves there too.
- Fixed: a finished download left behind after its title was removed (Meet Joe Black) is cleaned up.
- Fixed: Play on a show now starts the episode you have, not the first one of the show. Edgerunners started downloading season 1 again while you had season 2.

## 0.11.5 (2026-11-05)
- New: Use graphics card for video, in Settings > General. Video that has to be converted, like x265 films on a TV, is converted on your graphics card instead of the processor, which can help stutter on a TV and leaves more for your games. It only shows in the app on this PC.

## 0.11.4 (2026-11-05)
- New: Limit downloads, in Settings > General. All downloads together use at most 10% of your internet, up and down, so games on this PC stay smooth. It only shows in the app on this PC, never on a phone or TV.
- New, with it: Don't limit what I start here. Downloads you start in the app on this PC run at full speed, and ones started anywhere else stay limited.
- Pressing Watch now on something already downloading now switches that download to start-to-end order, so it plays as it comes instead of stopping at missing pieces.
- Watch now no longer sits on "Getting the first minutes" with most of the film downloaded. If the opening pieces get stuck on someone slow, Chillarr restarts the download so they come from someone else.
- Watch now opens faster: Jellyfin no longer spends minutes reading the download before it starts.
- If your phone loses touch with your PC while waiting, the screen now says so and keeps trying.
- On iPhones that won't start a video by themselves, the waiting screen now clears so you can press play.

## 0.11.3 (2026-11-05)

- Watch now no longer fails with "Jellyfin isn't answering". Jellyfin can't open a title until the start of it has downloaded, and Chillarr gave up after 20 seconds. It now stays on the waiting screen until the title can open, on every device.
- Jellyfin being slow now says it's busy, instead of saying it isn't answering.
- Night mode works on iPhone and on TVs. The first time you turn it on for a film, Chillarr makes an evened-out copy of the sound in a few minutes while you keep watching, then switches to it. Files you're still downloading get it once they're done.
- The subtitles and audio menu on a phone is now three short pages (Subtitles, Audio, Speed). Size and the other controls stay in view, and sideways it sits beside the picture.

## 0.11.2 (2026-11-04)
- Settings are much simpler. The everyday options come first, and technical ones are folded under More options or listed under Advanced. Nothing was taken away.
- System is now Status, and it just tells you if everything is working. The list of apps opens by itself if something breaks.
- Jellyfin & streaming is now Account & devices, with how to watch on your TV or phone near the top. Indexers is now Download sites.
- Explanations are shorter and use plainer words.
- OpenSubtitles is now a choice at the end of setup: Set it up or Skip, with a link to make a free account. You can still set it up later in Settings.

## 0.11.1 (2026-11-04)
- Tabs open at once. Each one shows what it had last time, even right after starting Chillarr, and updates what changed a moment later. Every tab is also made ready in the background shortly after starting.
- Everything loads faster: Chillarr asks your apps for things side by side instead of one after another, shares answers instead of asking for the same list over and over, and only works out what the tab you opened needs. Animations are quicker too, so nothing is held back from view.
- Fix everything no longer restarts Radarr every time. Radarr kept showing nekoBT and SubsPlease as failing because they're anime sites with no new movies to list, and restarting it for that made it slower each time, until it could stall for 15 seconds and show "Not running" while it was running. A restart now really closes and starts the app, and happens at most once in 6 hours.
- A log file that grew huge because of that (2 GB on one PC) is cleared by itself when Chillarr starts.
- Stopping a download clears "Downloading 0%" from its poster. It used to stay there until you restarted Chillarr.
- A site that's down, like YTS right now, no longer counts as something for you to fix. Its card says the site is down and your other sites still work, and Chillarr stops testing it every 15 minutes in the background. Those tests were keeping your apps busy and making the site's retry wait longer.
- An app that's busy says "Slow to answer" instead of "Not running".

## 0.11.0 (2026-11-03)
- Setting up a new PC is one screen and one press. Pick a folder, a login, Subtitles or Dubbed, and a colour, press Set up Chillarr, and it does the rest: every download, every install, starting the apps and connecting them.
- One big meter shows all of it: how much has downloaded out of the total, the speed, the time left, and a line for each app. Broken connections pick up where they stopped.
- Windows asks for permission once instead of once per app. That one prompt covers the installs, the firewall rules (no more "Allow access?" windows) and Defender's protected folders.
- Chillarr's own installer is no longer a wizard. Double-click it, a short progress window runs, and the app opens.
- Sites are added by themselves. If one is down or blocked, Chillarr tries again later without being asked, up to every 6 hours. One you take out of Prowlarr stays out.
- Choose original voices with subtitles, or Dubbed in the language you pick (English, Spanish, French, German, Japanese and more), in Settings > General. It decides which copy gets downloaded and which audio a title starts in, when the file has both. Copies with both voices are picked first when the quality is the same, so the player's audio button can switch between them.
- Pick your colour in Settings > Appearance: seven themes or any colour you like, and a pure black background. The whole app follows, and so do the logo, the tray icon and your desktop and taskbar shortcuts.
- New hover cards: the poster stays under your mouse and the details open beside it, with the trailer above them.
- Series and anime show what you've watched clearly: a Watched label on each episode, a count for each season, and buttons to mark an episode or a whole season as watched or not watched. Marks stay after the files are deleted, even right after you finish an episode.
- Only Space or the pause button pauses now. Clicking the picture just brings the controls back. TV remotes keep their OK and play buttons.
- One icon in the tray: Chillarr's. Sonarr, Radarr and qBittorrent run without their own. The tray menu has "Stop everything and quit" for closing them all.
- Fix everything is faster, since apps start and get checked side by side, and it shows each step as it goes, then what it changed and how long it took.
- Starting up is quicker. The window opens straight away instead of waiting for every app to answer, and your apps start the moment Chillarr does instead of 30 seconds later, side by side.
- Apps that wouldn't start: if an app can't open its own files, Fix everything repairs that with one Windows prompt. One stuck without answering gets restarted, and if an app still won't start, Logs shows the app's own error instead of only "didn't answer".
- Prowlarr no longer runs as a Windows service. If it stopped, say for an update, every search failed until someone pressed Fix everything and answered a Windows prompt. Now Chillarr just starts it again, and moving all the apps over takes one prompt.
- Watch now on a profile limited to certain libraries no longer stops at "Jellyfin answered with error 404": it's let into Watch now's hidden library.
- Settings > Logs has a Save logs button: one file with what Chillarr did, how each app is doing, and each app's own log, with keys and passwords hidden. Chillarr's log also records every error Jellyfin, Sonarr, Radarr and Prowlarr answer with, and each step of Watch now.
- Adding several sites no longer makes Prowlarr's sync clash with itself in Sonarr and Radarr.
- Watch now: pressing Back while it's still looking for a copy really cancels, and a film asked for that way doesn't download later anyway. Removing a title mid-start keeps it removed.
- Watch now: stopping partway no longer marks it watched, your place is no longer lost when you leave during a reload or right after Start over, a paused season pack starts again for the episode you're on, Next stays full screen, the spinner clears after a seek while paused, and a stalled stream gives up with a message instead of spinning forever.
- The status dots on the app cards in Settings show the right colour. They were always grey.

## 0.10.0 (2026-10-30)
- Switching tabs is instant. Going back to Movies, Anime or any tab you've already opened shows it straight away, scrolled where you left it, and refreshes it quietly in the background. Only what actually changed gets redrawn.
- Remove from Continue watching: a title you won't finish can be taken off the row from its page or its card.
- Press ? in the player (or the new button in its bar) for a list of every shortcut.
- Seeking with the arrow keys, a TV remote or a controller now shows the preview picture too, not only with the mouse.
- The audio language you pick carries on to the next episode, the way subtitles already did.
- Each profile keeps its own player settings: subtitles, volume, night mode, quality and sound delay.
- "Skip full-disc copies" no longer moves a quality profile's target. A profile that allowed 4K but aimed for 1080p was switched to aim for 4K, so Radarr started upgrading films nobody asked to upgrade.
- Stopping a Discover title partway no longer counts as watching it, even if its download finished while you watched. Before, the clean-up could delete it as finished.
- A show stopped right after its download started stays stopped instead of being downloaded again.
- Finished Watch now downloads give their disk space back.
- Fix everything no longer force-closes a qBittorrent that is still starting or uses another port, and when it does need a restart it closes it properly first. A settings file it can't read is left alone instead of being replaced by an almost empty one, and download folders with a comma in their name work.
- Blocked indexers (like 1337x or EZTV) now really try their other addresses before giving up.
- Anime filters by tag show titles that are really about that tag, like the tag rows do. Chillarr also stays under AniList's limit, so anime rows stop failing to load in busy moments.
- Trickplay turns on for libraries added later, not only the ones there on the first run.
- The clean-up says so when a file couldn't be deleted, instead of reporting it as cleaned.
- Settings, profiles, history and the Discover shelf are saved in a way a crash or power cut can't damage. If the profiles file is lost anyway, Chillarr opens locked and asks for the main profile's Jellyfin sign-in instead of dropping every PIN. PIN lockouts now survive a restart.
- Safer inside Jellyfin: someone signed in on the web can only delete titles they can see, and the phone remote searches as whoever is signed in on that TV, never with the PC owner's account. Each Jellyfin account can have up to 5 phone-remote screens, and one device can no longer use up every connection and lock the others out.
- On LG TVs: the big banner on Home opens its title, search backgrounds show, picking a Similar title keeps its buttons, subtitles look clean, and Chillarr no longer slowly uses more memory the longer it stays open.
- Buttons in Settings no longer get stuck on "Testing..." or stay greyed out after something fails, and a page that can't load shows Try again instead of loading forever.
- The player no longer doubles its progress reports when you switch tracks or episodes quickly, and the TV remote's Back closes an open menu over the player before the player itself.

## 0.9.2 (2026-10-28)
- My downloads no longer says "Nothing downloaded yet" when you do have things. If Jellyfin was still starting (it takes about half a minute) or too busy to answer, the page took that as an empty library. Now it says it's getting your library and keeps trying until Jellyfin answers.

## 0.9.1 (2026-10-22)
- The TV remote works in Chillarr on LG TVs. Arrows and OK did nothing before: LG's Jellyfin app never handed the remote to Chillarr's page, and older TVs send the keys in a form Chillarr didn't read. Now the arrows move from the first poster and OK opens it.
- Every TV has its own phone remote code. On the TV, open Settings > Phone remote, then type that code on your phone. The phone then controls that TV and only that TV. Each TV keeps its code until you press New code on it. The code in Chillarr on the PC still controls the PC.
- The phone tells you which screen it's controlling, and says so if that TV isn't showing Chillarr right now.
- The phone remote works over Tailscale too. Settings lists your Tailscale address next to the Wi-Fi one, and opening the remote by the Tailscale name no longer gets turned away.
- On the PC, Settings > General > Phone remote lists each TV with its code, and Forget turns one off.

## 0.9.0 (2026-10-21)
- Chillarr now lives inside Jellyfin. Open Jellyfin in a browser, on your phone (the Jellyfin app) or in Jellyfin Media Player, sign in, and you land on Chillarr's home instead of Jellyfin's: the same rows, Top 10, hover cards with trailers, filters, search, My List, Download, Watch now and the player with night mode, subtitle download and sync, skip intro and Up next. Everyone gets their own My List, Continue watching and Watched marks, tied to their Jellyfin account.
- Profiles inside Jellyfin: tap your avatar for "Who's watching?", pick a profile (its PIN if it has one) and the page switches to it, no signing out. Add a profile right there too. A new profile can see no more than the profile that made it, so a kids' profile can't make an unlimited one.
- The main profile asks for its Jellyfin password the first time a phone or browser switches into it, then remembers that device for 180 days. Settings > Jellyfin & streaming > Forget browsers makes them all ask again (a lost phone, a lent laptop).
- Made for phones and tablets too: a bottom tab bar, nothing cut off at the sides, bigger buttons, swipe through the top picks, and a player that works by touch. In the Jellyfin iPhone and Android apps the back gesture works inside Chillarr, and the player fills the screen.
- Jellyfin's own pages (sign-in, "Who's watching?", your settings and the dashboard) got Chillarr's dark look and font all the way through.
- Who may do what comes from Jellyfin: anyone signed in can watch, download and save to My List. Deleting files and stopping downloads needs Jellyfin's "Allow media deletion". Storage and library upkeep is for Jellyfin admins. Fixing apps, installing and changing folders stays in the Chillarr app on the PC.
- On LG TVs (Jellyfin's webOS app) Chillarr is the whole app, remote and all: arrows and OK everywhere, Back steps out (and leaves the app from Home), play, pause, stop and skip keys work in the player, and the number keys type a profile's PIN. Big posters, a clear highlight on whatever is picked, and lighter effects so older TVs don't lag. Older LG TVs (from 2018 on) get a version of the pages made for their browser, by themselves.
- Pictures keep their shape on every screen: films play letterboxed instead of stretched (wide, old 4:3 and phone-shot vertical video alike), subtitles stay on the picture, and posters and backgrounds crop instead of squashing, on TVs, phones turned either way, tablets and PCs.
- Starting and stopping something on the Discover shelf before it has downloaded no longer marks it watched. Jellyfin can't tell how long those are, so it took any stop as "watched to the end", and once the film arrived it showed under Completed (and the clean-up could have deleted it). Chillarr now takes that mark back within 30 seconds; a real watched mark on something you have is never touched.
- Phones and browsers keep their own names in Jellyfin's device list (an iPhone used to show up as "Windows").
- Removing a title from your Watch now list is allowed for everyone, even without Jellyfin's "Allow media deletion": it only frees what your own Watch now downloaded and nobody else uses.
- Switching profiles inside Jellyfin uses Jellyfin's Quick Connect. If it's off, Chillarr turns it on the first time someone opens the profile picker (it was already on here).
- TV apps that don't use Jellyfin's web pages (Android TV, Roku, Apple TV) keep Jellyfin's own look.
- Works on Jellyfin's http address (like http://your-pc:8096 or your Tailscale name). An https address in front of Jellyfin isn't supported yet.

## 0.8.3 (2026-10-21)
- The buttons on a hover card now do what they say instead of opening the title's page. Resume and Play start playing (a show plays the episode you're at), and Download on a film starts downloading straight away, in the quality you last picked. Download on a show still opens its page, so you can pick which episodes.

## 0.8.2 (2026-10-21)
- Clicking just below the progress bar (between the buttons) no longer pauses the film. Only a click on the picture itself does.
- The bars stay up while your mouse is on them, like YouTube, instead of fading after 3 seconds. Any mouse move brings them back.
- Skip and volume notes are bigger and stay up for 3 seconds. Skips keep adding up while the note is showing ("Forward 30 seconds", with where you are now), and the volume note has a level bar like VLC's.

## 0.8.1 (2026-10-20)
- Subtitles now line up with the film by themselves. Chillarr listens to the film's sound once and moves the subtitles to match, even ones made for another copy of the film (like Replicas, where one file was 36 seconds late and another was made for a 25 fps copy). Files next to the video are fixed on disk for good, so Jellyfin on the TV gets the fixed timing too. Subtitles Chillarr downloads by itself come already fixed.
- Picking a newly downloaded subtitle now really shows it. Before, the player could keep showing the old file after a download.
- Fix subtitle timing by hand: Timing - and + in the subtitles menu, or G and H while watching (like VLC). There's also a Sync to the audio button. Chillarr remembers your timing for that file.
- Full screen no longer loses the mouse: moving it brings the controls back, like it does in a window.
- Click anywhere on the picture to pause or play, with a big pause or play sign in the middle like Jellyfin's. Double click for full screen.
- Skipping says how far you went ("+30 s" when you press it three times), and Skip intro says how much it skipped. Volume changes show on screen, like VLC.
- Volume goes up to 300%, like VLC, without crackling. Use the slider, the arrow keys or the mouse wheel. M mutes. Your volume is kept for next time.
- Hovering the progress bar shows the time you'd jump to, with a little picture of that moment like in Jellyfin. Chillarr turns Jellyfin's preview pictures on for your Movies, Series and Anime; Jellyfin makes them in the background, so they show up film by film. The number keys jump to 10%, 20% and so on, and J and L skip back and forward.
- Night mode (in the subtitles and audio menu, or N): quiet talking gets louder and explosions softer, so you don't have to keep changing the volume.
- Sound delay, in the same menu: when voices come before the lips move, push the sound later. Chillarr remembers it for that title.
- Playing something you stopped part way asks "Resume from 42:10" or "Start over", like Jellyfin. A movie's own Resume and Start over buttons don't ask again.
- Esc closes the menu first, then full screen, then the player.
- A movie you searched for and then downloaded (like Replicas) no longer also shows up in Jellyfin's Discover shelf, so the TV doesn't list it twice.

## 0.8.0 (2026-10-20)
- Filters for Movies and Series now go as deep as Anime's. Movies: language, out now or coming soon, age rating, length and studio. Series: language, still running or ended, episode length, kind of show, network or streaming service, and country.
- Korean in one click: the new Language row on Filters shows only Korean shows (by language or by a Korean network), or leaves them out with a second click. Japanese, Chinese, Thai and Turkish work the same way. The new details fill in over the next day, so some titles show up later.
- The search box shows your recent searches in Chillarr's own look, one list per profile, instead of Windows' plain white box. Remove one with the x, or clear them all.
- Anime that isn't downloaded yet (like Bleach) now lists its episodes and offers Watch now.
- Stop and Remove buttons are as wide as the poster above them.
- Profiles with a PIN are properly locked. Changing or removing a locked profile needs its PIN. Forgot it? Delete the profile with your Chillarr login. Wrong PINs make you wait a little longer each time, and Chillarr opens locked until you pick a profile.
- Sonarr, Radarr and Prowlarr ask for your login again, also on your home Wi-Fi. Before, anyone on your Wi-Fi could open them without a password.
- The phone remote has a new, longer code. Pair your phone once more with the code in Settings. Wrong codes only lock out the device typing them, not your phone.
- Before letting apps past Defender's protected-folder rule, Chillarr asks first and names them. Turn the question back on in Settings > General.
- Plugins and helpers come from fixed, checked versions only (Intro Skipper, Open Subtitles, FlareSolverr), and Chillarr checks its own files before running them.
- Your Jellyfin sign-in and profile sign-ins are now saved encrypted, so other Windows accounts or a copied file can't read them.

## 0.7.7 (2026-10-19)
- New rows on the Anime page: Ecchi, Harem, Supernatural and School, next to Action, Romance, Comedy and Fantasy. Harem only shows shows that really are harem ones (High School DxD, Quintessential Quintuplets, Nisekoi), not every show with a hint of it.
- Opening Jellyfin at localhost:8096 in a browser now shows Chillarr's look and search, the same as the LAN and Tailscale addresses. A browser that had opened it before kept showing its old saved copies of Jellyfin's logo and search page. It now refreshes them once, by itself.

## 0.7.6 (2026-10-19)
- Skip dubbed copies now catches exactly the dubbed ones (like Hindi + English or Italian + English) and nothing else. In 0.7.5 it could flag plain English copies too.

## 0.7.5 (2026-10-19)
- New setting, on by default: Skip dubbed copies. Downloads leave out copies with an extra dubbed language mixed in (like "Dual Audio Hindi English"), so nothing starts in the wrong language. A film's own language is always fine, and so is anime with Japanese and English.
- Skip full-disc copies now also skips oversized normal copies of movies: about 8 GB at most for a two-hour film in 1080p, 20 GB in 4K. Whole seasons are judged per episode, so season packs still come.

## 0.7.4 (2026-10-19)
- New setting, on by default: Skip full-disc copies. Downloads never take a remux or a whole Blu-ray disc (25 to 80 GB for one film, looks the same on a TV). They take a normal copy of a few GB instead, everywhere: Download, Watch now and requests from the TV. Turn it off in Settings > General to allow remuxes again.
- Films you already have are left alone.

## 0.7.3 (2026-10-19)
- Next episode and Up next always offer the show's real next episode. If it isn't downloaded yet, it plays as it downloads ("Downloads as you watch"), and only that episode downloads. It crosses into the next season, and hides when the next episode isn't out yet.
- Back takes you to exactly where you were: the same page, the same spot on it, the same row position and the card you opened. Closing the player or a download finishing no longer throws you back to the top.
- The Back buttons on Explore and Filters, and the controller's B, return to where you were too.
- When Jellyfin's new look is switched on, Chillarr waits for Jellyfin to finish starting before checking it worked, so it no longer reports a false problem.

## 0.7.2 (2026-10-19)
- Jellyfin now looks like Chillarr in the browser and the phone apps: our colours, font and logo, rounded cards that lift on hover, a cinematic page for each title, a matching sign-in page and player. It's still a normal Jellyfin server, so every Jellyfin app finds it. (The Android TV app draws its own screens and keeps its look.)
- Searching in Jellyfin always searches everything. Searching from inside Movies used to miss titles on the Discover shelf (Iron Man) until you pressed "Retry with a global search".
- The first time this version runs, Chillarr restarts Jellyfin once to switch the new look on, only when nobody is watching.
- Chillarr's Watch now library no longer shows up in Jellyfin's menu and home page.
- Loading looks much better: the page shows the shape of what's coming (the big banner with its title and buttons, rows of posters across the whole width) with a soft light passing over it, instead of a flat grey block.
- Watch now on a show opens its episode list so you pick the episode. Every show lists its episodes, even ones you don't have yet.
- Watch now no longer fails with "error 400" while Jellyfin is busy scanning, and starts much faster (seconds instead of a minute of waiting on Jellyfin).
- Tokyo Toshokan works again: it now sits behind Cloudflare, so it goes through FlareSolverr. Any site that needs FlareSolverr gets it automatically, also ones added earlier.
- The Pirate Bay no longer shows as failing in Radarr. Its movie feed is always empty, which Radarr's test counted as a failure, and every Fix everything made Radarr skip it for longer. Searching it works and is kept.

## 0.7.1 (2026-10-19)
- On your TV or phone, shows on the Discover shelf now list their real episodes. Playing one gets only that episode, for series and anime alike. The show stays on the shelf so you can keep going episode by episode, and episodes you already have drop off it.
- Playing an episode of a show you stopped earlier gets just that episode, nothing it wanted before.
- The first start after updating takes a while in Jellyfin: it adds every shelf episode once.

## 0.7.0 (2026-10-19)

### Search
- Searching in Jellyfin's own apps (TV, phone) now finds what you type. Titles you search for that aren't on the Discover shelf yet are added within seconds, and the search index updates right after every shelf change instead of hours later.
- When one movie of a series is on the shelf, the rest of the series comes with it (Iron Man brings Iron Man 2 and 3).
- A title you stopped earlier goes back on the shelf instead of disappearing from Jellyfin.
- Search in Chillarr shows results as they come in: your own titles first, then everything else, with no waiting on the slowest source.
- A new search page: the top result's picture in the background, a title count, and Movies / Series / Anime chips.
- Every result shows its genres (High School DxD no longer says "No genre").

### Downloads
- Picking some seasons or episodes gets only those. New seasons don't start downloading by themselves.
- Anything you delete stays deleted. Sonarr and Radarr no longer download it again.
- Things that aren't out yet are searched for when they come out, then again after 1, 3, 6 and 12 hours, then daily, until a copy is found. This keeps going across restarts.

### Subtitles
- Download has a Subtitles choice per title, in any of 43 languages. English is picked for you when the title isn't in English.
- Subtitles are fetched when the file arrives, and looked for again for two weeks if none exist yet. They come from OpenSubtitles when you sign in (Settings), otherwise from free sources.
- Deleting a title or episodes deletes their subtitles too.

### My List > Completed
- Everything you finish shows up, counted per episode and season, including things you streamed before downloading.
- Each title says when it gets cleaned up ("Deletes in 2 days"), or "Deleted" once it's gone. It stays on the list after that, so you know you watched it.
- Titles you finished show "Watched" everywhere, and Download asks before getting one again.
- Taking a title off Completed only lasts until you finish more of it.

### Browsing
- Pressing the big banner at the top opens that title.
- "Similar to this" at the bottom of every title, from real recommendations, and it stays up to date. Click one to open it, Back to return.
- A new hover card: squarer, with a bigger title, genres and a smoother entrance.
- Titles without a poster get a proper card instead of a grey box.
- Filters explain what Season means for anime, with a Year next to it ("Fall 2025").
- Smoother animations across the app: loading, pages, the detail panel, toasts.

### Full screen
- Full screen now fills the whole monitor, not just the Chillarr window. In the player, and for the whole app with F11 or the new button in the top bar.

### Setup
- Setup has a new, optional Subtitles step: sign in to OpenSubtitles (or make a free account from there) so missing subtitles can be found, from Chillarr and from your TV or phone.
- The OpenSubtitles sign-in starts with your Chillarr name filled in.
