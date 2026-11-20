//! Spacing, heights and radii, and the sizes of the shared controls (`components`).

pub const SPACE_4: f32 = 4.0;
pub const SPACE_8: f32 = 8.0;
pub const SPACE_12: f32 = 12.0;
pub const SPACE_16: f32 = 16.0;
pub const SPACE_24: f32 = 24.0;
pub const SPACE_32: f32 = 32.0;
pub const INSET: f32 = 12.0;
pub const BAR_H: f32 = 36.0;
pub const SLIM_BAR_H: f32 = 30.0;
pub const ROW_H: f32 = 26.0;
pub const GRID_HEADER_H: f32 = 30.0;
pub const BUTTON_H: f32 = 28.0;
pub const RADIUS_CONTROL: u8 = 2;
pub const RADIUS_FLOAT: u8 = 4;
pub const SPINNER: f32 = 14.0;

/// A boxed button (`button/box`): 12 between its box and its words, 8 between its words and its
/// shortcut.
pub const BUTTON_PAD_X: f32 = 12.0;
pub const BUTTON_GAP: f32 = 8.0;
/// A text button (`button/text`): 10 either side of its words.
pub const TEXT_BUTTON_PAD_X: f32 = 10.0;
/// A dialog (`dialog`): this wide, its content this far inside its 1 px edge; this between its
/// head, its body and its buttons; this between its title and its text; this between the parts
/// of its body (a field, a checkbox); its buttons at least this wide, this far apart.
pub const DIALOG_W: f32 = 440.0;
pub const DIALOG_PAD: i8 = 20;
pub const DIALOG_GAP: f32 = 20.0;
pub const DIALOG_HEAD_GAP: f32 = 6.0;
pub const DIALOG_BODY_GAP: f32 = 12.0;
pub const DIALOG_BUTTON_MIN_W: f32 = 80.0;
pub const DIALOG_BUTTON_GAP: f32 = 8.0;
/// An icon button (`button/icon`): a square of this side.
pub const ICON_BUTTON: f32 = 28.0;
/// A bar button (`button/bar`): this tall, the icon kind as wide; the label kind has 12 between
/// its ends and what it holds, 8 between its icon and its words, and a 1 px divider at its left.
pub const BAR_BUTTON_H: f32 = 36.0;
pub const BAR_BUTTON_PAD_X: f32 = 12.0;
pub const BAR_BUTTON_GAP: f32 = 8.0;
pub const BAR_BUTTON_DIVIDER: f32 = 1.0;

/// An input (`input`): this tall, its words this far inside its 1 px edge.
pub const INPUT_H: f32 = 28.0;
pub const INPUT_PAD_X: f32 = 8.0;
/// The text caret's width, in every text input and editor.
pub const CARET_W: f32 = 1.0;
/// A field (`field`): between its label and its input, and between its input and its error line.
pub const FIELD_GAP: f32 = 4.0;

/// A dropdown (`dropdown`): this tall, its words this far inside its 1 px edge, this between its
/// words and its chevron.
pub const DROPDOWN_H: f32 = 28.0;
pub const DROPDOWN_PAD_X: f32 = 8.0;
pub const DROPDOWN_GAP: f32 = 6.0;
/// A checkbox (`checkbox`): its box, and between the box and its label.
pub const CHECKBOX_BOX: f32 = 14.0;
pub const CHECKBOX_GAP: f32 = 8.0;
/// A segmented control (`segmented`): a box this tall, its segments this tall inside its 1 px
/// edge; a caps segment has this between its ends and its words; a segment ends in a 1 px slot
/// for the divider between it and the next.
pub const SEGMENTED_H: f32 = 28.0;
pub const SEGMENT_H: f32 = 26.0;
pub const SEGMENT_PAD_X: f32 = 12.0;
pub const SEGMENT_DIVIDER: f32 = 1.0;
/// A colour swatch (`swatch`): a square of this side, this far from the next.
pub const SWATCH: f32 = 20.0;
pub const SWATCH_GAP: f32 = 8.0;

/// A menu (`menu`): this far inside its 1 px edge.
pub const MENU_PAD: i8 = 4;
/// A menu's item (`menu/item`): this tall, its words this far in from either end; the chosen
/// mark's column is this wide, and this is between the column, the words, the keys and the
/// submenu's arrow, with this more before the keys.
pub const MENU_ITEM_H: f32 = 28.0;
pub const MENU_ITEM_PAD_X: f32 = 8.0;
pub const MENU_MARK: f32 = 14.0;
pub const MENU_ITEM_GAP: f32 = 8.0;
pub const MENU_KEYS_PAD: f32 = 16.0;
/// A menu's separator (`menu/separator`): its 1 px line has this above and below it.
pub const MENU_SEPARATOR_PAD: f32 = 4.0;
/// A menu's heading over its items (the database chip's list): this between it and the first.
pub const MENU_HEADING_GAP: f32 = 6.0;
/// A menu bar's title (`menubar/title`): this tall, its word this far in from either end.
pub const MENU_TITLE_H: f32 = 24.0;
pub const MENU_TITLE_PAD_X: f32 = 8.0;

/// A tooltip (`tooltip`): its words this far in from its outside edge, across and down; at most
/// this wide, its words wrapping inside; this far below the pointer.
pub const TOOLTIP_PAD_X: i8 = 8;
pub const TOOLTIP_PAD_Y: i8 = 4;
pub const TOOLTIP_W: f32 = 320.0;
pub const TOOLTIP_GAP: f32 = 4.0;

/// A toast (`toast`): this tall for one line of words (a wrapped one grows by its lines), its
/// parts this far in from its ends and this far apart; at most this wide, its words wrapping.
pub const TOAST_H: f32 = 44.0;
pub const TOAST_PAD_X: f32 = 12.0;
pub const TOAST_GAP: f32 = 12.0;
pub const TOAST_MAX_W: f32 = 480.0;
/// A toast's action: a box this tall, its words this far in from its ends.
pub const TOAST_ACTION_H: f32 = 28.0;
pub const TOAST_ACTION_PAD_X: f32 = 12.0;
/// A toast's close: the square round its glyph that a click reaches.
pub const TOAST_CLOSE: f32 = 28.0;
/// Where toasts stand: this far from the window's right edge and above what they sit on (the
/// status bar), and this far apart.
pub const TOAST_MARGIN: f32 = 12.0;
pub const TOAST_STACK_GAP: f32 = 8.0;

/// A tab (`tab`): this tall, the strip too; its icon, title, dot and close this far in from its
/// ends and this far apart; its title cut at this width; the unsaved dot this wide; the square
/// round the close's glyph that a click reaches; the active line this tall along its top.
pub const TAB_H: f32 = 36.0;
pub const TAB_PAD_X: f32 = 12.0;
pub const TAB_GAP: f32 = 8.0;
pub const TAB_TITLE_MAX: f32 = 200.0;
pub const TAB_DOT: f32 = 8.0;
pub const TAB_CLOSE: f32 = 18.0;
pub const TAB_ACTIVE_LINE: f32 = 2.0;
/// The new tab button (`tab/new`): this wide, as tall as a tab; the list button (`tab/list`) the
/// same. The list button's menu opens this far under it.
pub const NEW_TAB_W: f32 = 32.0;
pub const TAB_LIST_GAP: f32 = 2.0;

/// The work area with no tab open (empty state C1): a block this wide holding the phrase, then
/// this far under it the ways to start, one a row, this far apart.
pub const NO_TAB_W: f32 = 220.0;
pub const NO_TAB_GAP: f32 = 16.0;
pub const NO_TAB_ROW_GAP: f32 = 8.0;

/// The menu bar (`menu bar`): a slim bar, its titles this far in from its ends.
pub const MENU_BAR_PAD_X: f32 = 4.0;

/// The status bar (`status bar`): a slim bar with the connection's colour this thick along its
/// top; the name's block this tall, the name this far in from its ends and cut at this width;
/// this between the block and the first fact; this between the facts, and between an activity's
/// words, its timer and its CANCEL; this between the bar's other parts; its right end this far
/// in. IN TRANSACTION's square mark is this wide.
pub const STATUS_EDGE: f32 = 2.0;
pub const STATUS_BLOCK_H: f32 = 28.0;
pub const STATUS_BLOCK_PAD_X: f32 = 12.0;
pub const STATUS_NAME_MAX: f32 = 360.0;
pub const STATUS_AFTER_BLOCK: f32 = 20.0;
pub const STATUS_FACT_GAP: f32 = 12.0;
pub const STATUS_GAP: f32 = 8.0;
pub const STATUS_PAD_X: f32 = 12.0;
pub const TRANSACTION_MARK: f32 = 8.0;

/// A bar with a line along its bottom (`run row`, `result bar`, `table bar`): the line this thick;
/// what stands in the bar (a result tab, a bar button) is this tall, above it.
pub const BAR_LINE: f32 = 1.0;
pub const BAR_CELL_H: f32 = BAR_H - BAR_LINE;
/// The run row (`run row`): its buttons this far in from its left end and this far apart.
pub const RUN_ROW_PAD_X: f32 = 12.0;
pub const RUN_ROW_GAP: f32 = 4.0;
/// What leads the run row (the database chip) stands this far before RUN.
pub const CHIP_RUN_GAP: f32 = SPACE_8;
/// Short of room, the run row's database chip narrows to this, its name cut.
pub const RUN_CHIP_MIN_W: f32 = 96.0;
/// The query editor (`editor`): its line numbers' box this far in from its left, its text this
/// far after the box and under its top.
pub const EDITOR_INSET: f32 = 12.0;
/// The split between the query editor and the run row: a zone this tall centred on the row's top
/// line takes the drag.
pub const SPLIT_ZONE_H: f32 = SPACE_8;
/// A result tab (`result tab`): its title this far in from its ends, cut at this width.
pub const RESULT_TAB_PAD_X: f32 = 12.0;
pub const RESULT_TAB_TITLE_MAX: f32 = 200.0;
/// The result bar (`result bar`): its facts this far apart, and the last this far before EXPORT
/// or the value panel's toggle.
pub const RESULT_FACT_GAP: f32 = 12.0;
/// The row count (`rows/count`): this between its words and COUNT; while counting, this between
/// its words, COUNTING and the timer.
pub const ROW_COUNT_GAP: f32 = 8.0;
pub const ROW_COUNT_COUNTING_GAP: f32 = 12.0;
/// The table bar (`table bar`): its view switch this far in from its left end; this between the
/// switch and the first cell, and between the count and IMPORT; CANCEL this far in its cell, either
/// side; LOADING this far before its timer.
pub const TABLE_BAR_PAD_X: f32 = 12.0;
pub const TABLE_BAR_GAP: f32 = 12.0;
pub const TABLE_CANCEL_PAD_X: f32 = 12.0;
pub const TABLE_LOADING_GAP: f32 = 12.0;
/// The filter bar (`filter row`, on `card`): its rows this far in, above and below them and either
/// side; each row this tall, its controls this far apart.
pub const FILTER_BAR_PAD_Y: f32 = 4.0;
pub const FILTER_BAR_PAD_X: f32 = 12.0;
pub const FILTER_ROW_H: f32 = 36.0;
pub const FILTER_GAP: f32 = 8.0;
/// The paging bar (`paging bar`, its line along the top): its arrows this far in from its left
/// end, its last button this far from its right end, its parts this far apart; the staged mark
/// this big and this far before the count of changes, which ends this far before RESET.
pub const PAGING_PAD_LEFT: f32 = 8.0;
pub const PAGING_PAD_RIGHT: f32 = 12.0;
pub const PAGING_GAP: f32 = 4.0;
pub const STAGED_MARK: f32 = 8.0;
pub const STAGED_MARK_GAP: f32 = 8.0;
pub const CHANGES_GAP: f32 = 12.0;
/// The paging bar's note on a cut page: this far after LAST, and shown only with this much room
/// (cut with `…` to it).
pub const PAGING_NOTE_GAP: f32 = 12.0;
pub const PAGING_NOTE_MIN: f32 = 24.0;
/// The table tab's note on rows that can't be edited: an area this tall (its 1 px line along the
/// bottom included), its sentence this far in from either side.
pub const NOTE_BAR_H: f32 = 28.0;
pub const NOTE_BAR_PAD_X: f32 = 12.0;
/// The table tab's words where the grid would be (`Loading …`, `No rows are loaded.`): this far in
/// from the grid area's left and down from its top.
pub const TABLE_EMPTY_PAD: f32 = 12.0;
/// The sidebar's header (`sidebar/filter`, `sidebar/count`): the filter is a bar with its line
/// along the bottom, its words this far in from the left, the refresh this far from its right end;
/// the count's row is this tall, its words this far in.
pub const SIDEBAR_PAD_X: f32 = 12.0;
pub const SIDEBAR_PAD_RIGHT: f32 = 4.0;
pub const SIDEBAR_COUNT_H: f32 = 28.0;
/// The sidebar (`sidebar`): this wide when a window opens, its 1 px `line` down the right edge
/// included, with no margin round what it holds; a drag keeps it within these widths.
pub const SIDEBAR_W: f32 = 260.0;
pub const SIDEBAR_MIN_W: f32 = 200.0;
pub const SIDEBAR_MAX_W: f32 = 600.0;

/// A query tab's body: the grid runs edge to edge in it; what stands above and below the grid
/// (the bars, the notes, the error band) keeps this far in from its sides. The table tab's areas
/// run the body's full width.
pub const TAB_BODY_PAD_X: f32 = SPACE_8;

/// The grid's cells (`grid/header cell`, `grid/cell`, `grid/row number`): what a cell holds is
/// this far in from its sides; a header's parts are this far apart, the two arrows of its sort
/// hint this far; the row numbers are at least this wide; a selected cell's outline is this thick;
/// the follow link stands this far after the text; the failed mark is a triangle this big in the
/// top right corner.
pub const GRID_CELL_PAD_X: f32 = 8.0;
pub const GRID_HEADER_GAP: f32 = 6.0;
pub const GRID_SORT_HINT_GAP: f32 = 1.0;
pub const GRID_ROW_NUMBER_MIN_W: f32 = 36.0;
pub const GRID_OUTLINE_W: f32 = 2.0;
pub const GRID_LINK_GAP: f32 = 6.0;
pub const GRID_FAILED_MARK: f32 = 6.0;

/// A tree row (`tree/row`): a row's height; what it holds starts this far in, one level this much
/// further; the chevron's square, then the icon's, then the name, this far apart; the type ends
/// this far from the right. The chevron answers a click in a square this big round its own.
pub const TREE_ROW_H: f32 = ROW_H;
pub const TREE_PAD_X: f32 = 12.0;
pub const TREE_INDENT: f32 = 16.0;
pub const TREE_CHEVRON: f32 = 12.0;
pub const TREE_ICON: f32 = 14.0;
pub const TREE_GAP: f32 = 6.0;
pub const TREE_EXPANDER: f32 = 20.0;
/// A connection row (`sidebar/connection`): this tall, the connection's colour this wide along its
/// left edge, its engine's logo this far in from either end in a square this big, its words this
/// far after the logo; keyboard focus this far inside it.
pub const CONNECTION_ROW_H: f32 = 40.0;
pub const CONNECTION_COLOUR_W: f32 = 3.0;
pub const CONNECTION_PAD_X: f32 = 12.0;
pub const CONNECTION_LOGO: f32 = 14.0;
pub const CONNECTION_LOGO_GAP: f32 = 8.0;
pub const CONNECTION_FOCUS_INSET: f32 = 1.0;
/// The connection screen's sidebar: the connections file's warning, under the filter, has this
/// much room above and below its words (and [`SIDEBAR_PAD_X`] at its sides).
pub const CONN_WARNING_PAD_Y: f32 = 8.0;
/// The connection screen's right pane: the connection's colour is an edge this thick along the
/// top of the title row; the title gets no narrower than this before FILL FROM URL loses its
/// words; the engine row's logo is this big, this far before the engine's name.
pub const CONN_COLOUR_EDGE: f32 = 2.0;
pub const CONN_TITLE_MIN_W: f32 = 80.0;
pub const ENGINE_ROW_LOGO: f32 = 16.0;
pub const ENGINE_ROW_GAP: f32 = SPACE_8;
/// The connection form: two columns when the pane is at least this wide, one below; a column at
/// least this wide (two of them, the gap and the pane's insets make the two-column width) and at
/// most this wide, the second this far right of the first. The first section starts this far
/// under the engine row's line; a section this far under the field above it, a field this far
/// under the one above (and under its section's label); a port field this wide; a button this
/// far beside its field and the next button.
pub const CONN_TWO_COLUMNS_MIN: f32 = 664.0;
pub const CONN_COLUMN_MIN_W: f32 = 300.0;
pub const CONN_COLUMN_MAX_W: f32 = 480.0;
pub const CONN_COLUMN_GAP: f32 = 24.0;
pub const CONN_FIELDS_TOP: f32 = SPACE_16;
pub const CONN_SECTION_GAP: f32 = 20.0;
pub const CONN_FIELD_GAP: f32 = SPACE_12;
pub const CONN_PORT_W: f32 = 110.0;
pub const CONN_BESIDE_GAP: f32 = SPACE_8;
/// The form's bar while a test or connect has run 300 ms: the facts' word and the timer this far
/// apart, and the timer this far before the first button.
pub const CONN_RUNNING_GAP: f32 = SPACE_12;
/// The engine cards (K2): `Choose a database` this far under the title row, the cards this far
/// under it; a card this tall, at least this wide, its logo this big, this far in from the card's
/// left and top, the name this far under the logo; the UPCOMING chip this far in from the corner.
pub const ENGINE_HEADING_TOP: f32 = SPACE_16;
pub const ENGINE_CARDS_TOP: f32 = SPACE_12;
pub const ENGINE_CARD_H: f32 = 104.0;
pub const ENGINE_CARD_MIN_W: f32 = 110.0;
pub const ENGINE_CARD_LOGO: f32 = 28.0;
pub const ENGINE_CARD_PAD: f32 = 16.0;
pub const ENGINE_CARD_NAME_GAP: f32 = SPACE_12;
pub const ENGINE_CARD_CHIP_INSET: f32 = SPACE_8;

/// The structure view (`structure/heading`, `structure/header cell`, `structure/cell`): a section's
/// heading this tall, its words this far in; the header this tall; a row this tall; what a cell
/// holds this far in from its sides, a column's position this far from the left; a key's icon this
/// far before its words, a foreign key's arrow this far after the column it points at.
pub const STRUCTURE_HEADING_H: f32 = BAR_H;
pub const STRUCTURE_HEADING_PAD_X: f32 = 12.0;
pub const STRUCTURE_HEADER_H: f32 = SLIM_BAR_H;
pub const STRUCTURE_ROW_H: f32 = ROW_H;
pub const STRUCTURE_CELL_PAD_X: f32 = 8.0;
pub const STRUCTURE_POSITION_PAD_X: f32 = 12.0;
pub const STRUCTURE_ICON_GAP: f32 = 6.0;
pub const STRUCTURE_LINK_GAP: f32 = 4.0;

/// The value panel (`value panel`): the bar's words, the value and the facts this far in from its
/// sides, the value this far below the bar and above the facts; the bar's parts (the column's
/// name and type, COPY and WRAP) this far apart; the facts in a slim bar along the bottom, this far apart; CANCEL and SAVE this far
/// apart; the editing caret this tall in its row. Short of room the name keeps at least
/// `VALUE_NAME_MIN` before COPY and WRAP lose their words, each then an icon in a cell
/// `VALUE_ICON_CELL_W` wide (a bar button's square).
pub const VALUE_INSET: f32 = 12.0;
pub const VALUE_BAR_GAP: f32 = 6.0;
pub const VALUE_FACTS_H: f32 = SLIM_BAR_H;
pub const VALUE_FACT_GAP: f32 = 12.0;
pub const VALUE_EDIT_GAP: f32 = 8.0;
pub const VALUE_CARET_H: f32 = 16.0;
pub const VALUE_NAME_MIN: f32 = 64.0;
pub const VALUE_ICON_CELL_W: f32 = BAR_BUTTON_H;
/// The value panel's width when it first shows, beside the tab's results.
pub const VALUE_PANEL_W: f32 = 340.0;
/// The tab's results keep at least this beside the value panel: the panel narrows first, and
/// isn't drawn when that would leave it narrower than it can be made.
pub const RESULTS_MIN_W: f32 = 320.0;

/// The error band (`error band`): its edge down the left this wide; what it holds this far in from
/// its sides, past the edge; its parts this far apart, with this much more above a field (HINT,
/// DETAIL) and above the note; a field's name this far over its text; the heading and the
/// position at least this far apart.
pub const ERROR_BAND_EDGE: f32 = 2.0;
pub const ERROR_BAND_PAD: f32 = 12.0;
pub const ERROR_BAND_GAP: f32 = 4.0;
pub const ERROR_BAND_FIELD_GAP: f32 = 8.0;
pub const ERROR_BAND_NAME_GAP: f32 = 2.0;
pub const ERROR_BAND_HEAD_GAP: f32 = 12.0;
/// The band's actions row (R2): this far under its last text, its buttons this far apart; the
/// position is drawn only while the heading keeps at least this much room (gap 68).
pub const ERROR_BAND_ACTIONS_GAP: f32 = 12.0;
pub const ERROR_BAND_ACTION_GAP: f32 = 8.0;
pub const ERROR_BAND_HEADING_MIN: f32 = 120.0;
/// The strip (a result or a note, as the connection form shows one above its bar): its edge down
/// the left this wide; its words this far under its top line and over its bottom; at most this
/// many lines of them in view, the rest scrolling.
pub const STRIP_EDGE: f32 = 2.0;
pub const STRIP_PAD_Y: f32 = SPACE_8;
pub const STRIP_MAX_LINES: usize = 3;
/// The quick search (QC with R4's rows): a box this wide (less where the window is narrower,
/// keeping this margin from its sides and its bottom), this far below the window's top, its
/// content this far inside; a field this tall, then this gap before the list, whose headings and
/// rows are all this tall. In a row and in the field, an icon's or a mark's centre sits this far
/// from the left and the words start this far in, ending this far from the right; a connection's
/// name stops this far before its database type's label; a table's schema takes at most this
/// share of the room. A connection's mark is a square of this side in a row, of this side in a
/// heading; a heading's mark (or its words, with none) starts this far in, its words this far
/// after the mark, their line's centre this far below the heading's top.
pub const QS_W: f32 = 640.0;
pub const QS_TOP: f32 = 56.0;
pub const QS_PAD: i8 = 12;
pub const QS_MARGIN: f32 = 12.0;
pub const QS_FIELD_H: f32 = 40.0;
pub const QS_LIST_GAP: f32 = 4.0;
pub const QS_ROW_H: f32 = 32.0;
pub const QS_ICON_X: f32 = 15.0;
pub const QS_TEXT_X: f32 = 30.0;
pub const QS_END_PAD: f32 = 8.0;
pub const QS_KIND_GAP: f32 = 12.0;
pub const QS_MARK: f32 = 8.0;
pub const QS_HEADING_MARK: f32 = 6.0;
pub const QS_HEADING_X: f32 = 8.0;
pub const QS_HEADING_MARK_GAP: f32 = 10.0;
pub const QS_HEADING_LINE_Y: f32 = 19.0;
pub const QS_SCHEMA_SHARE: f32 = 0.4;
/// The editor (`editor/line number`, `editor/error token`): a line number's box is this wide, the
/// number flush with its right; an error's underline is this thick, this far down its line.
pub const LINE_NUMBER_W: f32 = 36.0;
pub const ERROR_UNDERLINE_H: f32 = 1.5;
pub const ERROR_UNDERLINE_Y: f32 = 17.0;

pub const KEYCAP_H: f32 = 18.0;
/// Between the keycap's edge and its key, and between the keys of one shortcut.
pub const KEYCAP_PAD_X: f32 = 4.0;
pub const KEYCAP_GAP: f32 = 3.0;
pub const CHIP_H: f32 = 18.0;
/// Between the chip's edge and what it holds.
pub const CHIP_PAD_X: f32 = 6.0;
/// The available chip's square mark, and the gap after it.
pub const CHIP_MARK: f32 = 6.0;
pub const CHIP_GAP: f32 = 6.0;

/// Keyboard focus (foundations' Focus rule): a 2 px `ink` edge or box, or a 1 px `on_ink` line.
pub const FOCUS_W: f32 = 2.0;
pub const FOCUS_LINE_W: f32 = 1.0;
/// How far the box around a control with no edge reaches out from it.
pub const FOCUS_AROUND: f32 = 4.0;
/// How far a bar button's box and the `on_ink` line sit inside the control.
pub const FOCUS_INSET: f32 = 2.0;
pub const RADIUS_FOCUS_AROUND: u8 = 4;
pub const RADIUS_FOCUS_LINE: u8 = 1;
