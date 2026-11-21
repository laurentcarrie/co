//! The help window, in French. It is shown from the "Aide" button of every screen.

use eframe::egui;

/// The screens that have a part of the help.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Page {
    Menu,
    Calibration,
    Viewer,
}

/// (screen, title, paragraphs).
const SECTIONS: &[(Page, &str, &[&str])] = &[
    (
        Page::Menu,
        "Menu de démarrage",
        &[
            "Ce programme pose la trace GPS de votre montre sur la carte de la course, puis vous la montre tronçon par tronçon.",
            "Carte : le fichier de la carte, en PDF ou en image numérisée (PNG, JPEG, TIFF). La carte doit avoir le nord en haut. Le bouton « Parcourir… » ouvre le sélecteur de fichiers.",
            "Trace : le fichier .tcx exporté par la montre (Garmin).",
            "Date : lue dans la trace, au format AAAA-MM-JJ.",
            "Nom de la carte : à saisir, par exemple « rocher dame jouanne ». Il est proposé à partir du nom du fichier de la carte s'il est de la forme « date-nom ». Le bouton « Deviner… » demande à OpenStreetMap le nom du village près de la trace : il envoie sur le réseau une seule position, le milieu de la trace arrondi à environ 100 m, et seulement si vous cliquez dessus.",
            "Enregistrer dans : le dossier où sera rangé le fichier de la course. Il est créé s'il n'existe pas.",
            "La course est enregistrée dans un seul fichier zip, nommé « date-nom » : par exemple 2026-11-15-rocher-dame-jouanne.zip. Il contient la carte (map.pdf), la trace de la montre (race.tcx) et la transformation (transformation.yml) : où sont placés le triangle, les postes et l'arrivée, les tours ignorés ou déplacés. Les fichiers d'origine ne sont pas modifiés, ils sont copiés dans le zip.",
            "« Démarrer » ouvre la calibration. Le nombre de postes est proposé d'après les cercles vus sur la carte : corrigez-le si besoin.",
            "« Ouvrir une course enregistrée (.zip)… » reprend une course déjà enregistrée : la carte, la trace et vos choix sont relus, et on arrive directement à la visualisation.",
        ],
    ),
    (
        Page::Calibration,
        "Calibration : placer le triangle, les postes et l'arrivée",
        &[
            "Le programme doit savoir où est la trace sur la carte. Il faut lui indiquer, sur la carte, la position du triangle de départ, de chaque poste et de l'arrivée.",
            "Au démarrage, il cherche les cercles imprimés sur la carte et propose un premier placement. Vérifiez chaque point, surtout l'ordre des postes quand deux cercles sont proches, et corrigez ce qui est faux.",
            "Pour placer ou déplacer un point : cliquez sur sa ligne dans la liste (Triangle, 1, 2, … Arrivée), puis cliquez sur son emplacement sur la carte. Le point suivant à placer est alors sélectionné. Le bouton ✕ retire un point. Les boutons + et − ajoutent ou retirent un poste.",
            "Dès que le triangle et l'arrivée sont placés, le tracé apparaît sur la carte, et il est mis à jour à chaque poste placé. Avec seulement le triangle et l'arrivée, c'est une estimation grossière. L'erreur moyenne et l'erreur maximale sont données en mètres : une grande erreur indique souvent un point mal placé.",
            "La trace entière est dessinée en gris clair, y compris ce qui précède le triangle et ce qui suit l'arrivée.",
            "Souris : la molette zoome, le glisser-déposer déplace la carte.",
        ],
    ),
    (
        Page::Calibration,
        "Les tours de la montre",
        &[
            "Un poste est atteint quand vous avez appuyé sur le bouton Tour de la montre : chaque tronçon se termine à un appui sur Tour, et non au point de la trace le plus proche du poste (on peut passer près d'un poste sans y être).",
            "Le programme relie les postes, dans l'ordre, aux appuis sur Tour qui leur correspondent le mieux. La liste « Tours de la montre » donne chaque appui, son heure et le poste auquel il est relié. L'arrêt de la montre compte comme un dernier appui, pour l'arrivée.",
            "Un appui fait par erreur n'est relié à aucun poste : il est « inutilisé ». Si le programme relie un poste au mauvais appui, décochez cet appui dans la liste, ou faites un clic droit dessus sur la carte : les postes sont alors reliés de nouveau sans lui.",
            "Sur la carte, un appui est un point blanc avec le numéro de son poste, un point gris avec « ? » s'il est inutilisé, un point rouge avec « ✕ » s'il est ignoré. Passez la souris dessus pour le détail.",
            "Pour déplacer un point placé (triangle, poste ou arrivée) quand son cercle n'est pas au bon endroit, glissez-le sur la carte : le tracé est recalé automatiquement quand vous relâchez. Un point sert aussi de poignée pour un appui sur Tour qui est au même endroit : maintenez Maj pour déplacer l'appui au lieu du point.",
            "Pour corriger l'endroit d'un appui (utilisé, inutilisé ou ignoré), glissez son point le long de la trace : il reste entre les deux appuis voisins, et le calage est refait quand vous relâchez. « Remettre les tours à leur place » annule ces déplacements.",
            "S'il y a moins d'appuis que de postes, chaque poste est relié au point de la trace le plus proche, et le panneau le signale.",
        ],
    ),
    (
        Page::Calibration,
        "Le triangle de départ",
        &[
            "Deux cas, selon la case « J'ai appuyé sur Tour au triangle ».",
            "Case cochée : le triangle est comme le poste 0, relié à un appui sur Tour (le premier, avant le poste 1). L'heure de passage au triangle est alors connue, et le tronçon 1 commence à cet appui.",
            "Case décochée : le triangle marque seulement le début du tronçon 1, et vous donnez l'heure vous-même. Cliquez sur « Quand suis-je passé au triangle ? » : une fenêtre s'ouvre avec un curseur qui va du départ de la montre à l'arrivée au poste 1. Placez-le à l'instant où vous étiez au triangle : le point de la trace à cet instant est posé sur le triangle, et le tronçon 1 est redessiné à partir de là. Si le triangle lui-même est mal placé, cliquez sur « Placer le triangle » puis sur la carte. « Terminé » enregistre, « Annuler » garde le départ automatique.",
        ],
    ),
    (
        Page::Calibration,
        "Enregistrer et ouvrir la visionneuse",
        &[
            "Quand tous les points sont placés, « Enregistrer » écrit le fichier zip de la course (carte, trace et transformation), dans le dossier choisi. On reste sur l'écran de calibration.",
            "Le bouton « VISUALISATION » en haut à gauche affiche les tronçons : durée, distance, allure, fréquence cardiaque. Quand des modifications ne sont pas encore enregistrées, on en est prévenu.",
            "Dans la visionneuse, le bouton « ÉDITION » revient à la calibration : vos points et les tours ignorés sont conservés.",
        ],
    ),
    (
        Page::Viewer,
        "Visionneuse",
        &[
            "La liste « Tronçons » à gauche permet de choisir un tronçon : la carte zoome dessus. « Tous les tronçons » montre tout.",
            "Pour le tronçon choisi : la durée, la distance courue, la distance à vol d'oiseau (ligne droite entre les postes), leur rapport et l'allure moyenne.",
            "« Colorier selon » colorie le tronçon choisi selon l'allure ou la fréquence cardiaque. « Afficher les autres tronçons » les garde en transparence.",
            "Pour l'allure et la fréquence cardiaque, une barre à deux poignées règle les extrémités de l'échelle de couleurs (min et max) ; une poignée ne peut pas dépasser l'autre ; « Réinitialiser l'échelle » revient aux valeurs de la course. Le curseur « Épaisseur du tracé » règle la largeur de la ligne.",
            "Le curseur en bas, quand un tronçon est choisi, se déplace le long du tronçon en mètres : un point jaune se déplace sur la carte, et l'heure, la distance, l'allure, l'altitude et la fréquence cardiaque s'affichent, avec le profil d'altitude.",
            "Molette : zoom. Glisser-déposer : déplacer la carte. « Ajuster à la fenêtre » réaffiche toute la carte.",
            "« Quand suis-je passé au triangle ? » ouvre la fenêtre qui donne l'heure du passage au triangle (voir la calibration).",
        ],
    ),
];

/// Shows the help window while `open`. The part for `page` is open at first.
pub fn window(ctx: &egui::Context, open: &mut bool, page: Page) {
    egui::Window::new("Aide")
        .open(open)
        .default_width(620.0)
        .default_height(560.0)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (section_page, title, paragraphs) in SECTIONS {
                    egui::CollapsingHeader::new(egui::RichText::new(*title).strong())
                        .id_salt(format!("{title}-{page:?}"))
                        .default_open(*section_page == page)
                        .show(ui, |ui| {
                            for text in *paragraphs {
                                ui.label(*text);
                                ui.add_space(4.0);
                            }
                        });
                }
            });
        });
}
