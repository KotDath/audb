import 'dart:convert';
import 'dart:io';
import 'package:flutter/material.dart';
void main() => runApp(const MaterialApp(home: Probe()));
class Probe extends StatefulWidget {
 const Probe({super.key});
 @override State<Probe> createState() => _ProbeState();
}
class _ProbeState extends State<Probe> {
 final first = TextEditingController();
 final second = TextEditingController();
 void save() {
  final root = Platform.environment['XDG_DATA_HOME'] ?? '${Platform.environment['HOME']}/.local/share';
  final dir = Directory('$root/audb-input-probe')..createSync(recursive: true);
  File('${dir.path}/state.json').writeAsStringSync(jsonEncode({'first':first.text,'second':second.text,'selectionStart':first.selection.start,'selectionEnd':first.selection.end}));
 }
 @override void initState() { super.initState(); first.addListener(save); second.addListener(save); }
 @override Widget build(BuildContext context) => Scaffold(appBar:AppBar(title:const Text('AUDB input probe (temporary)')),
  body:Padding(padding:const EdgeInsets.all(30),child:Column(children:[
   TextField(controller:first,autofocus:true,minLines:3,maxLines:5,decoration:const InputDecoration(labelText:'First / multiline')),
   TextField(controller:second,decoration:const InputDecoration(labelText:'Second / focus cancellation')),
   TextButton(onPressed:(){first.clear();second.clear();},child:const Text('Clear both')),
  ])));
}
