//! The interface's words in English and Russian.

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Language {
    English,
    Russian,
}

impl Language {
    pub fn other(self) -> Language {
        match self {
            Language::English => Language::Russian,
            Language::Russian => Language::English,
        }
    }
}

macro_rules! strings {
    ($($id:ident: $english:literal, $russian:literal;)*) => {
        #[derive(Clone, Copy, PartialEq, Eq)]
        pub enum Str {
            $($id),*
        }

        const TABLE: &[(&str, &str)] = &[$(($english, $russian)),*];
    };
}

strings! {
    Title: "3D Reversi", "Реверси 3D";
    BlackToMove: "Black to move", "Ход чёрных";
    WhiteToMove: "White to move", "Ход белых";
    Thinking: "Computer is thinking…", "Компьютер думает…";
    Draw: "Draw", "Ничья";
    BlackWins: "Black wins", "Чёрные победили";
    WhiteWins: "White wins", "Белые победили";
    BoardFull: "the board is full", "доска заполнена";
    NoMovesLeft: "neither side can move", "ходов больше нет";
    BlackPasses: "Black has no move and passes", "Чёрным некуда ходить — пас";
    WhitePasses: "White has no move and passes", "Белым некуда ходить — пас";
    Pass: "pass", "пас";
    You: "You", "Вы";
    Computer: "Computer", "Компьютер";
    Player: "Player", "Игрок";
    Black: "Black", "Чёрные";
    White: "White", "Белые";
    Moves: "Moves", "Ходы";
    NoMovesYet: "No moves yet", "Ходов пока нет";
    Undo: "Undo", "Назад";
    Redo: "Redo", "Вперёд";
    Hint: "Hint", "Подсказка";
    NewGame: "New game", "Новая партия";
    Flip: "Flip board", "Развернуть";
    Mode: "Mode", "Режим";
    ModeBlack: "You play Black", "Вы за чёрных";
    ModeWhite: "You play White", "Вы за белых";
    ModeTwoPlayers: "Two players", "Два игрока";
    ModeDemo: "Demo game", "Демонстрация";
    Level: "Level", "Уровень";
    Level1: "Beginner", "Новичок";
    Level2: "Casual", "Любитель";
    Level3: "Club player", "Разрядник";
    Level4: "Expert", "Сильный";
    Level5: "Master", "Мастер";
    ShowMoves: "Show moves", "Показ ходов";
    Theme: "Theme", "Тема";
    Sound: "Sound", "Звук";
    On: "On", "Вкл";
    Off: "Off", "Выкл";
    LanguageName: "English", "Русский";
    Footer: "F1 help · Tab panel · F11 full screen", "F1 справка · Tab панель · F11 весь экран";
    Controls: "Controls", "Управление";
    KeyClick: "Click", "Щелчок";
    KeyDrag: "Drag", "Перетаскивание";
    KeyWheel: "Wheel", "Колесо мыши";
    HelpClick: "Place a disc on a marked square", "Поставить фишку на отмеченное поле";
    HelpRotate: "Turn the view (either mouse button)", "Повернуть вид (любой кнопкой мыши)";
    HelpZoom: "Zoom in and out", "Приблизить или отдалить";
    HelpUndo: "Take back", "Отменить ход";
    HelpRedo: "Replay", "Вернуть ход";
    HelpHint: "Suggest a move", "Подсказать ход";
    HelpNew: "New game", "Новая партия";
    HelpFlip: "Flip the board", "Развернуть доску";
    HelpTopView: "View from above", "Вид сверху";
    HelpLevel: "Computer level", "Уровень компьютера";
    HelpMode: "Who plays whom", "Кто с кем играет";
    HelpShowMoves: "Show or hide the legal moves", "Показать/скрыть возможные ходы";
    HelpTheme: "Next theme", "Следующая тема";
    HelpSound: "Sound on or off", "Звук вкл/выкл";
    HelpLanguage: "English / Russian", "Русский / английский";
    HelpPanel: "Show or hide the panel", "Показать/скрыть панель";
    HelpFullScreen: "Full screen", "Весь экран";
    HelpCopy: "Copy the game, Shift: the position", "Копировать партию, Shift: позицию";
    HelpPaste: "Paste a game or a position", "Вставить партию или позицию";
    HelpClose: "Close this help", "Закрыть справку";
    Rules: "Rules", "Правила";
    RulesNote: "Place a disc so that a line of your opponent's discs lies between it and one of yours: they all turn to your color. Black begins; a side that cannot move passes. The most discs at the end wins.",
        "Поставьте фишку так, чтобы между ней и вашей фишкой оказался ряд фишек соперника, — они перевернутся в ваш цвет. Начинают чёрные; кому некуда ходить, пропускает ход. Побеждает тот, у кого в конце больше фишек.";
    GameCopied: "Game copied", "Партия скопирована";
    PositionCopied: "Position copied", "Позиция скопирована";
    GameLoaded: "Game loaded", "Партия загружена";
    NothingToPaste: "The clipboard has no game or position", "В буфере нет партии или позиции";
    HintToast: "Hint: ", "Подсказка: ";
    NothingToUndo: "No moves to take back", "Нечего отменять";
    NotLegal: "Nothing to turn over there", "Сюда нельзя: нечего переворачивать";
}

impl Str {
    /// The name of a computer level from 1 to 5.
    pub fn level(level: i32) -> Str {
        [Str::Level1, Str::Level2, Str::Level3, Str::Level4, Str::Level5][(level.clamp(1, 5) - 1) as usize]
    }
}

pub fn tr(id: Str, language: Language) -> &'static str {
    let (english, russian) = TABLE[id as usize];
    match language {
        Language::English => english,
        Language::Russian => russian,
    }
}
